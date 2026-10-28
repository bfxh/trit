//! 权重容器与 BN 折叠。
//!
//! ckpt 由导出侧写成 `weights.bin` + `weights.json`（名字 / 偏移 / 形状索引），
//! 这里只做两件事：按名字取张量、把 BN 折叠进卷积权重与偏移。
//! 三值形态与稠密形态共用同一份折叠后的 bias，两者的差就是量化损失本身。

use std::collections::HashMap;
use std::path::Path;

/// BN 折叠的数值稳定项。
pub const EPS: f32 = 1e-5;

pub struct Tensors {
    pub data: HashMap<String, (Vec<f32>, Vec<usize>)>,
}

impl Tensors {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let idx = std::fs::read_to_string(dir.join("weights.json")).map_err(|e| e.to_string())?;
        let raw = std::fs::read(dir.join("weights.bin")).map_err(|e| e.to_string())?;
        let mut data = HashMap::new();
        for blk in idx.split('{').skip(1) {
            let blk = blk.split('}').next().unwrap_or("");
            let name = field_str(blk, "name").ok_or("weights.json 缺 name")?;
            let off: usize = field_num(blk, "off").ok_or("weights.json 缺 off")?;
            let n: usize = field_num(blk, "n").ok_or("weights.json 缺 n")?;
            let shape = field_shape(blk).ok_or("weights.json 缺 shape")?;
            let bytes = raw
                .get(off * 4..(off + n) * 4)
                .ok_or_else(|| format!("{name}: 越界"))?;
            let mut v = Vec::with_capacity(n);
            for c in bytes.chunks_exact(4) {
                v.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
            data.insert(name, (v, shape));
        }
        Ok(Self { data })
    }

    pub fn get(&self, name: &str) -> Result<&(Vec<f32>, Vec<usize>), String> {
        self.data
            .get(name)
            .ok_or_else(|| format!("缺张量 {name}"))
    }

    pub fn v(&self, name: &str) -> Result<&[f32], String> {
        Ok(&self.get(name)?.0)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

fn field_str(blk: &str, key: &str) -> Option<String> {
    let k = format!("\"{key}\":");
    let i = blk.find(&k)? + k.len();
    let rest = &blk[i..];
    let a = rest.find('"')? + 1;
    let b = rest[a..].find('"')? + a;
    Some(rest[a..b].to_string())
}

fn field_num(blk: &str, key: &str) -> Option<usize> {
    let k = format!("\"{key}\":");
    let i = blk.find(&k)? + k.len();
    let rest = blk[i..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn field_shape(blk: &str) -> Option<Vec<usize>> {
    let k = "\"shape\":";
    let i = blk.find(k)? + k.len();
    let rest = &blk[i..];
    let a = rest.find('[')? + 1;
    let b = rest.find(']')?;
    Some(
        rest[a..b]
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect(),
    )
}

/// 一层卷积的两种权重形态 + 折叠后的偏移。
pub struct ConvW {
    /// 稠密权重 `[(ky*kk+kx)*cin + ic][cout]`，已折叠 BN。
    pub wf: Vec<f32>,
    /// 三值权重，布局同上。
    pub wt: Vec<i32>,
    /// 每输出通道 scale（含 BN）。
    pub ws: Vec<f32>,
    /// 折叠后的偏移。
    pub bias: Vec<f32>,
    pub cin: u32,
    pub cout: u32,
    pub kk: u32,
}

/// 3x3 / 1x1 卷积 + BN（可选）。BN 折叠成每通道 scale 与偏移。
pub fn conv_w(t: &Tensors, wname: &str, bn: Option<&str>, relu: bool) -> Result<ConvW, String> {
    let (w, shape) = t.get(wname)?;
    if shape.len() != 4 {
        return Err(format!("{wname} 形状不是 4 维: {shape:?}"));
    }
    let (cout, cin, kh, kw) = (shape[0], shape[1], shape[2], shape[3]);
    let kk = kh;
    if kh != kw {
        return Err(format!("{wname} 非方核 {kh}x{kw}"));
    }
    let bias_raw = t.v(&wname.replace(".weight", ".bias"))?;
    if bias_raw.len() != cout {
        return Err(format!("{wname} bias 长度不符"));
    }
    let _ = relu;

    let (scale, shift) = match bn {
        Some(p) => {
            let g = t.v(&format!("{p}.weight"))?;
            let b = t.v(&format!("{p}.bias"))?;
            let mean = t.v(&format!("{p}.running_mean"))?;
            let var = t.v(&format!("{p}.running_var"))?;
            let mut s = vec![0f32; cout];
            let mut sh = vec![0f32; cout];
            for c in 0..cout {
                let sc = g[c] / (var[c] + EPS).sqrt();
                s[c] = sc;
                sh[c] = b[c] - mean[c] * sc;
            }
            (s, sh)
        }
        None => (vec![1f32; cout], vec![0f32; cout]),
    };

    let n = kk * kk * cin * cout;
    let mut wf = vec![0f32; n];
    let mut wt = vec![0i32; n];
    let mut ws = vec![0f32; cout];

    for oc in 0..cout {
        // 每输出通道的三值 scale：该通道 |w| 的均值
        let mut acc = 0f32;
        for ic in 0..cin {
            for ky in 0..kk {
                for kx in 0..kk {
                    acc += w[((oc * cin + ic) * kk + ky) * kk + kx].abs();
                }
            }
        }
        let s = acc / (cin * kk * kk) as f32;
        let s = if s > 1e-12 { s } else { 1.0 };
        ws[oc] = s * scale[oc];
        for ic in 0..cin {
            for ky in 0..kk {
                for kx in 0..kk {
                    let idx = (ky * kk + kx) * cin * cout + ic * cout + oc;
                    let wv = w[((oc * cin + ic) * kk + ky) * kk + kx];
                    wf[idx] = wv * scale[oc];
                    let q = (wv / s).round().clamp(-1.0, 1.0);
                    wt[idx] = q as i32;
                }
            }
        }
    }

    let mut bias = vec![0f32; cout];
    for oc in 0..cout {
        bias[oc] = bias_raw[oc] * scale[oc] + shift[oc];
    }

    Ok(ConvW {
        wf,
        wt,
        ws,
        bias,
        cin: cin as u32,
        cout: cout as u32,
        kk: kk as u32,
    })
}

/// ConvTranspose2d(k=2, stride=2)：无 BN。权重布局 `[(kh*2+kw)*cin + ci][cout]`。
pub fn convt_w(t: &Tensors, wname: &str) -> Result<ConvW, String> {
    let (w, shape) = t.get(wname)?;
    if shape.len() != 4 || shape[2] != 2 || shape[3] != 2 {
        return Err(format!("{wname} 不是 k=2 转置卷积: {shape:?}"));
    }
    let (cin, cout) = (shape[0], shape[1]);
    let n = 4 * cin * cout;
    let mut wf = vec![0f32; n];
    let mut wt = vec![0i32; n];
    let mut ws = vec![0f32; cout];
    for co in 0..cout {
        let mut acc = 0f32;
        for ci in 0..cin {
            for kh in 0..2 {
                for kw in 0..2 {
                    acc += w[((ci * cout + co) * 2 + kh) * 2 + kw].abs();
                }
            }
        }
        let s = acc / (cin * 4) as f32;
        let s = if s > 1e-12 { s } else { 1.0 };
        ws[co] = s;
        for ci in 0..cin {
            for kh in 0..2 {
                for kw in 0..2 {
                    let idx = (kh * 2 + kw) * cin * cout + ci * cout + co;
                    let wv = w[((ci * cout + co) * 2 + kh) * 2 + kw];
                    wf[idx] = wv;
                    wt[idx] = (wv / s).round().clamp(-1.0, 1.0) as i32;
                }
            }
        }
    }
    let bias_raw = t.v(&wname.replace(".weight", ".bias"))?;
    Ok(ConvW {
        wf,
        wt,
        ws,
        bias: bias_raw.to_vec(),
        cin: cin as u32,
        cout: cout as u32,
        kk: 2,
    })
}
