//! `weights.rs` 的 BN 折叠与布局转换（从 `weights.rs` 下沉）。
//!
//! 这里的每个形状都来自 `weights.json`，所以每处分配都以**真实张量长度**兜底：
//! 先把 `n == w.len()` 对上，`vec![…; n]` 才有可能安全。`w.len()` 由
//! `weights.bin` 的真实字节数决定，任何 json 声明都绕不过它。

use super::{ConvW, EPS, Tensors};

/// 形状相乘：任何一步溢出都报错，而不是回绕成一个小值让校验失效。
fn product(name: &str, parts: &[usize]) -> Result<usize, String> {
    parts
        .iter()
        .try_fold(1usize, |a, &b| a.checked_mul(b))
        .ok_or_else(|| format!("{name} 形状 {parts:?} 相乘溢出"))
}

/// 声明数与真实长度对账。
fn expect_len(name: &str, got: usize, want: usize) -> Result<(), String> {
    if got != want {
        return Err(format!("{name} 张量长度 {got} 与形状推出的 {want} 不符"));
    }
    Ok(())
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
    // 先算再比。`kk * kk * cin * cout` 裸乘在 64 位下能把 2^96 绕回 0，
    // 于是 `vec![0f32; 0]` 通过、循环里的 `w[…]` 才越界；不绕回而是真大时
    // 则是 `vec![0f32; n]` + `vec![0i32; n]` 两份按声明分配。
    let n = product(wname, &[kk, kk, cin, cout])?;
    expect_len(wname, w.len(), n)?;
    let bias_raw = t.v(&wname.replace(".weight", ".bias"))?;
    expect_len(&wname.replace(".weight", ".bias"), bias_raw.len(), cout)?;
    let _ = relu;

    let (scale, shift) = match bn {
        Some(p) => {
            let g = t.v(&format!("{p}.weight"))?;
            let b = t.v(&format!("{p}.bias"))?;
            let mean = t.v(&format!("{p}.running_mean"))?;
            let var = t.v(&format!("{p}.running_var"))?;
            // 四个张量都按 `cout` 取用，任何一个短了都会在 `g[c]` 越界。
            expect_len(&format!("{p}.weight"), g.len(), cout)?;
            expect_len(&format!("{p}.bias"), b.len(), cout)?;
            expect_len(&format!("{p}.running_mean"), mean.len(), cout)?;
            expect_len(&format!("{p}.running_var"), var.len(), cout)?;
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
    let n = product(wname, &[4, cin, cout])?;
    expect_len(wname, w.len(), n)?;
    let bias_raw = t.v(&wname.replace(".weight", ".bias"))?;
    expect_len(&wname.replace(".weight", ".bias"), bias_raw.len(), cout)?;
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

/// 把卷积有效权重转成「oc 外、ic 内」布局：`[(k·cout + oc)·cin + ic]`。
pub fn transpose_conv(eff: &[f32], cin: usize, cout: usize, kk: usize) -> Vec<f32> {
    let mut out = vec![0f32; eff.len()];
    for k in 0..kk * kk {
        for ic in 0..cin {
            for oc in 0..cout {
                out[(k * cout + oc) * cin + ic] = eff[(k * cin + ic) * cout + oc];
            }
        }
    }
    out
}

/// 转置卷积有效权重：`[(sub·cin + ci)·cout + co]` → `[(sub·cout + co)·cin + ci]`。
pub fn transpose_convt(eff: &[f32], cin: usize, cout: usize) -> Vec<f32> {
    let mut out = vec![0f32; eff.len()];
    for sub in 0..4usize {
        for ci in 0..cin {
            for co in 0..cout {
                out[(sub * cout + co) * cin + ci] = eff[(sub * cin + ci) * cout + co];
            }
        }
    }
    out
}
