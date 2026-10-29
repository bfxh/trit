//! `TPM1` 部署包：只装推理要用的东西（`q/scale/γ/β/bias`），不含 Adam/latent/RNG。
//!
//! 与训练存档 `TPNT3` 的区别：**体积小一个量级**（约 1.1 MB vs 7.5 MB）、只读、跨平台可直接加载。
//! 布局：`magic(8) | 版本(u32) | 层数(u32) | 每层{cin,cout,kk,relu,convt,n} | q[i8×n] |
//! scale/gamma/beta/bias[f32×cout]`。

use std::path::Path;

/// 一层部署参数。
#[derive(Clone)]
pub struct TpmLayer {
    pub q: Vec<i8>,
    pub scale: Vec<f32>,
    pub gamma: Vec<f32>,
    pub beta: Vec<f32>,
    pub bias: Vec<f32>,
    pub cin: u32,
    pub cout: u32,
    pub kk: u32,
    pub relu: bool,
    pub convt: bool,
}

impl TpmLayer {
    /// 有效权重 `E = q·scale`（布局与原模型一致）。
    pub fn effective(&self) -> Vec<f32> {
        let slots = self.cout as usize;
        self.q
            .iter()
            .enumerate()
            .map(|(wi, &qi)| qi as f32 * self.scale[wi % slots])
            .collect()
    }
}

pub struct TpmModel {
    pub layers: Vec<TpmLayer>,
}

impl TpmModel {
    /// 转成设备无关表示（`RefCpu` 用）。
    pub fn to_tri(&self) -> crate::model::TriModel {
        crate::model::TriModel {
            layers: self
                .layers
                .iter()
                .map(|l| crate::model::LayerW {
                    e: l.effective(),
                    bias: l.bias.clone(),
                    gamma: l.gamma.clone(),
                    beta: l.beta.clone(),
                    cin: l.cin,
                    cout: l.cout,
                    kk: l.kk,
                    relu: l.relu,
                    convt: l.convt,
                })
                .collect(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut b = Vec::new();
        b.extend_from_slice(b"TPM1\0\0\0\0");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(self.layers.len() as u32).to_le_bytes());
        for l in &self.layers {
            for v in [
                l.cin,
                l.cout,
                l.kk,
                l.relu as u32,
                l.convt as u32,
                l.q.len() as u32,
            ] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for &q in &l.q {
                b.push(q as u8);
            }
            for arr in [&l.scale, &l.gamma, &l.beta, &l.bias] {
                for v in arr {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, b)?;
        std::fs::rename(&tmp, path)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if b.len() < 16 || &b[0..8] != b"TPM1\0\0\0\0" {
            return Err("不是 TPM1 部署包".into());
        }
        let mut at = 8usize;
        let ver = take_u32(&b, &mut at)?;
        if ver != 1 {
            return Err(format!("TPM1 版本 {ver} 未支持"));
        }
        let n = take_u32(&b, &mut at)? as usize;
        let mut layers = Vec::with_capacity(n);
        for _ in 0..n {
            let cin = take_u32(&b, &mut at)?;
            let cout = take_u32(&b, &mut at)?;
            let kk = take_u32(&b, &mut at)?;
            let relu = take_u32(&b, &mut at)? != 0;
            let convt = take_u32(&b, &mut at)? != 0;
            let nw = take_u32(&b, &mut at)? as usize;
            let qs = b.get(at..at + nw).ok_or("TPM1 截断（q）")?;
            let q: Vec<i8> = qs.iter().map(|&v| v as i8).collect();
            at += nw;
            let mut take_f = |count: usize, b: &[u8], at: &mut usize| -> Result<Vec<f32>, String> {
                let mut v = vec![0.0f32; count];
                for x in &mut v {
                    *x = f32::from_bits(take_u32(b, at)?);
                }
                Ok(v)
            };
            let slots = cout as usize;
            let scale = take_f(slots, &b, &mut at)?;
            let gamma = take_f(slots, &b, &mut at)?;
            let beta = take_f(slots, &b, &mut at)?;
            let bias = take_f(slots, &b, &mut at)?;
            layers.push(TpmLayer {
                q,
                scale,
                gamma,
                beta,
                bias,
                cin,
                cout,
                kk,
                relu,
                convt,
            });
        }
        Ok(Self { layers })
    }
}

/// 从训练态（`Net`）导出。放在这里而不是 `net.rs`，是为了让 `TPM1` 的格式与训练态解耦。
#[cfg(feature = "gpu")]
pub fn from_net(net: &crate::net::Net) -> TpmModel {
    TpmModel {
        layers: net
            .layers
            .iter()
            .map(|l| TpmLayer {
                q: l.q.iter().map(|&v| v as i8).collect(),
                scale: l.scale.clone(),
                gamma: l.gamma.clone(),
                beta: l.beta.clone(),
                bias: l.bias.clone(),
                cin: l.cin,
                cout: l.cout,
                kk: l.kk,
                relu: l.relu,
                convt: l.convt,
            })
            .collect(),
    }
}

fn take_u32(b: &[u8], at: &mut usize) -> Result<u32, String> {
    let s = b.get(*at..*at + 4).ok_or("TPM1 截断")?;
    *at += 4;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tpm_roundtrip() {
        let m = TpmModel {
            layers: vec![TpmLayer {
                q: vec![1, 0, -1, 1],
                scale: vec![0.5, 0.25],
                gamma: vec![1.0, 1.0],
                beta: vec![0.0, 0.0],
                bias: vec![0.1, -0.2],
                cin: 2,
                cout: 2,
                kk: 1,
                relu: true,
                convt: false,
            }],
        };
        let dir = std::env::temp_dir().join("trit-parts-tpm");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.tpm");
        m.save(&p).unwrap();
        let back = TpmModel::load(&p).unwrap();
        assert_eq!(back.layers.len(), 1);
        assert_eq!(back.layers[0].q, vec![1, 0, -1, 1]);
        assert_eq!(back.layers[0].effective(), m.layers[0].effective());
        std::fs::remove_file(&p).ok();
    }
}
