//! 宽度扩展（net2net）：把窄模型的通道搬进放大的新网络后再精调。
//!
//! 独立成模块是为了守住「单文件 ≤800 行」的门。

use crate::net::{E1B, E2B, Layer, Net, U2, U3};

impl Net {
    /// 宽度扩展（net2net）：把窄模型的通道搬进按 `width` 放大的新网络。
    ///
    /// 只搬重叠的 `[k][ic][oc]` 块，新通道留 0（等价于"先不激活新通道"，后续精调再长出来）。
    /// **cat 层要分段搬**：`d2a` 的输入是 `[u3 | b]`、`d1a` 是 `[u2 | a]`，两端都变宽，
    /// 简单按前 N 个通道搬会把 b 的那一段错位。
    pub fn expand_width(&self, width: f32) -> Net {
        let mut new = Net::random_w(1, width);
        assert_eq!(new.layers.len(), self.layers.len());
        // cat 分段的旧/新边界（先算好，避免闭包借用 new）
        let old_seg = vec![
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some((
                self.layers[U3].cout as usize,
                self.layers[E2B].cout as usize,
            )),
            None,
            None,
            Some((
                self.layers[U2].cout as usize,
                self.layers[E1B].cout as usize,
            )),
            None,
            None,
        ];
        let new_seg = vec![
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some((new.layers[U3].cout as usize, new.layers[E2B].cout as usize)),
            None,
            None,
            Some((new.layers[U2].cout as usize, new.layers[E1B].cout as usize)),
            None,
            None,
        ];
        for i in 0..self.layers.len() {
            let old = &self.layers[i];
            let old_e = old.effective();
            let (oc_old, ic_old) = (old.cout as usize, old.cin as usize);
            let (oc_new, ic_new) = (new.layers[i].cout as usize, new.layers[i].cin as usize);
            let nk = if old.convt {
                4usize
            } else {
                (old.kk * old.kk) as usize
            };
            // 原样搬 latent / q（不要重算 scale，否则 E 会被重新量化而改变输出）
            let mut latent = vec![0f32; new.layers[i].latent.len()];
            let mut qv = vec![0i32; latent.len()];
            let seg = old_seg[i].zip(new_seg[i]);
            for k in 0..nk {
                for ic in 0..ic_old {
                    let dst_ic = match seg {
                        Some(((ca_old, _), (ca_new, _))) => {
                            if ic < ca_old {
                                ic
                            } else {
                                ca_new + (ic - ca_old)
                            }
                        }
                        None => ic,
                    };
                    if dst_ic >= ic_new {
                        continue;
                    }
                    for oc in 0..oc_old {
                        let src = (k * ic_old + ic) * oc_old + oc;
                        let dst = (k * ic_new + dst_ic) * oc_new + oc;
                        latent[dst] = old.latent[src];
                        qv[dst] = old.q[src];
                    }
                }
            }
            let l = &mut new.layers[i];
            for oc in 0..oc_old {
                l.scale[oc] = old.scale[oc];
                l.bias[oc] = old.bias[oc];
                l.gamma[oc] = old.gamma[oc];
                l.beta[oc] = old.beta[oc];
            }
            l.latent = latent;
            l.q = qv;
            let _ = &old_e;
        }
        new
    }

    /// 从部署包（`TPM1`）建网：`latent = q·scale`，`scale/q` 原样保留（E 逐位不变）。
    pub fn from_tpm(m: &crate::tpm::TpmModel) -> Self {
        let layers = m
            .layers
            .iter()
            .map(|l| {
                let slots = l.cout as usize;
                let n = l.q.len();
                Layer {
                    latent: l.effective(),
                    scale: l.scale.clone(),
                    q: l.q.iter().map(|&v| v as i32).collect(),
                    bias: l.bias.clone(),
                    cin: l.cin,
                    cout: l.cout,
                    kk: l.kk,
                    relu: l.relu,
                    grad_e: vec![0.0; n],
                    grad_b: vec![0.0; slots],
                    m_w: vec![0.0; n],
                    v_w: vec![0.0; n],
                    m_s: vec![0.0; slots],
                    v_s: vec![0.0; slots],
                    m_b: vec![0.0; slots],
                    v_b: vec![0.0; slots],
                    t: 0,
                    dev: None,
                    acc_e: None,
                    acc_b: None,
                    gamma: l.gamma.clone(),
                    beta: l.beta.clone(),
                    grad_gamma: vec![0.0; slots],
                    grad_beta: vec![0.0; slots],
                    m_g: vec![0.0; slots],
                    v_g: vec![0.0; slots],
                    m_be: vec![0.0; slots],
                    v_be: vec![0.0; slots],
                    dev_gb: None,
                    acc_gb: None,
                    dev_t: None,
                    convt: l.convt,
                    residual: false,
                }
            })
            .collect();
        Self { layers }
    }
}
