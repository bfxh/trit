//! 深度读出：冻结骨干的 d1 特征（32 通道）→ 逐像素视差（1/深度），f32 线性头。
//!
//! 这是「深度预测」的第一种实现（f32 基线）：只在主机侧拟合 `w/b`，
//! 目标用**视差**（1/z）而不是深度：近距离权重不会被远距离压平，数值更好。
//! 拟合用 IRLS（L1 回归），设计矩阵显式带截距列。

use std::path::Path;

/// 特征通道数（d1 输出）。
pub const CH: usize = 32;

/// 线性深度头，含特征标准化参数。
#[derive(Clone, Debug)]
pub struct DepthHead {
    pub w: Vec<f32>,
    pub b: f32,
    pub mu: Vec<f32>,
    pub sd: Vec<f32>,
}

impl DepthHead {
    pub fn zeros(ch: usize) -> Self {
        Self {
            w: vec![0.0; ch],
            b: 0.0,
            mu: vec![0.0; ch],
            sd: vec![1.0; ch],
        }
    }

    /// 用一批特征统计标准化参数。
    pub fn calibrate(&mut self, feats: &[f32], npix: usize) {
        let ch = self.mu.len();
        let inv = 1.0 / npix.max(1) as f64;
        for (c, (mu, sd)) in self.mu.iter_mut().zip(self.sd.iter_mut()).enumerate() {
            let mut s = 0.0f64;
            let mut s2 = 0.0f64;
            for p in 0..npix {
                let v = f64::from(feats[p * ch + c]);
                s += v;
                s2 += v * v;
            }
            let m = s * inv;
            let var = (s2 * inv - m * m).max(1e-12);
            *mu = m as f32;
            *sd = (var.sqrt() as f32).max(1e-3);
        }
    }

    /// 逐像素视差（标准化特征裁剪到 ±8）。
    pub fn disparity(&self, feats: &[f32], npix: usize) -> Vec<f32> {
        let ch = self.w.len();
        let mut out = vec![0.0f32; npix];
        for (p, slot) in out.iter_mut().enumerate() {
            let mut acc = self.b;
            for c in 0..ch {
                acc += self.w[c] * ((feats[p * ch + c] - self.mu[c]) / self.sd[c]).clamp(-8.0, 8.0);
            }
            *slot = acc;
        }
        out
    }

    /// 视差 → 深度（米，做安全裁剪）。
    pub fn depth(disp: &[f32]) -> Vec<f32> {
        disp.iter().map(|&d| 1.0 / d.clamp(0.05, 2.0)).collect()
    }

    /// IRLS 拟合（L1 回归）：`x` 是 n×CH 的**已标准化**特征，`y` 是视差。
    /// 设计矩阵显式带一列常数（截距），避免 bias 与特征共线时互相吸收。
    pub fn fit_irls(x: &[f32], n: usize, y: &[f32], ridge: f64, iters: usize) -> (Vec<f32>, f32) {
        let d = CH + 1;
        let mut w = vec![0f64; d];
        for _ in 0..iters.max(1) {
            // 平铺的 d×(d+1) 增广矩阵：行 u 的 [..d] 是正规方程，[d] 是右端。
            let mut a = vec![0f64; d * (d + 1)];
            for i in 0..n {
                let mut p = w[CH];
                for c in 0..CH {
                    p += w[c] * f64::from(x[i * CH + c]);
                }
                let r = f64::from(y[i]) - p;
                let wt = 1.0 / r.abs().max(0.005);
                for u in 0..d {
                    let run = wt
                        * if u == CH {
                            1.0
                        } else {
                            f64::from(x[i * CH + u])
                        };
                    for v in u..d {
                        let rv = if v == CH {
                            1.0
                        } else {
                            f64::from(x[i * CH + v])
                        };
                        a[u * (d + 1) + v] += run * rv;
                    }
                    a[u * (d + 1) + d] += run * f64::from(y[i]);
                }
            }
            for u in 0..d {
                for v in 0..u {
                    a[u * (d + 1) + v] = a[v * (d + 1) + u];
                }
                a[u * (d + 1) + u] += ridge;
            }
            // 高斯-约当消元（部分主元）。
            for col in 0..d {
                let mut piv = col;
                for row in col + 1..d {
                    if a[row * (d + 1) + col].abs() > a[piv * (d + 1) + col].abs() {
                        piv = row;
                    }
                }
                if piv != col {
                    for j in 0..=d {
                        a.swap(col * (d + 1) + j, piv * (d + 1) + j);
                    }
                }
                let dd = a[col * (d + 1) + col];
                if dd.abs() < 1e-12 {
                    continue;
                }
                for j in col..=d {
                    a[col * (d + 1) + j] /= dd;
                }
                for row in 0..d {
                    if row == col {
                        continue;
                    }
                    let f = a[row * (d + 1) + col];
                    if f == 0.0 {
                        continue;
                    }
                    for j in col..=d {
                        a[row * (d + 1) + j] -= f * a[col * (d + 1) + j];
                    }
                }
            }
            for (u, slot) in w.iter_mut().enumerate() {
                *slot = a[u * (d + 1) + d];
            }
        }
        (w[..CH].iter().map(|&v| v as f32).collect(), w[CH] as f32)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut s = String::with_capacity(4096);
        s.push_str("TPDH1\n");
        s.push_str(&format!("{}\n{}\n", self.w.len(), self.b));
        for ((w, m), d) in self.w.iter().zip(self.mu.iter()).zip(self.sd.iter()) {
            s.push_str(&format!("{w} {m} {d}\n"));
        }
        std::fs::write(path, s)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut it = s.lines();
        if it.next() != Some("TPDH1") {
            return Err("不是 TPDH1".into());
        }
        let ch: usize = it.next().and_then(|v| v.parse().ok()).ok_or("缺通道数")?;
        let b: f32 = it.next().and_then(|v| v.parse().ok()).ok_or("缺 bias")?;
        let mut head = DepthHead::zeros(ch);
        head.b = b;
        for c in 0..ch {
            let line = it.next().ok_or("行数不足")?;
            let v: Vec<f32> = line
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if v.len() != 3 {
                return Err("列数不足".into());
            }
            head.w[c] = v[0];
            head.mu[c] = v[1];
            head.sd[c] = v[2];
        }
        Ok(head)
    }
}

/// GT 深度 → 视差（非法像素给 0，配套 `valid` 使用）。
pub fn to_disparity(depth: &[f32]) -> Vec<f32> {
    depth
        .iter()
        .map(|&d| {
            if d.is_finite() && d > 0.05 && d < 50.0 {
                1.0 / d
            } else {
                0.0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(n: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut st = 12345u64;
        let mut rnd = || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st % 2001) as f32 / 1000.0 - 1.0
        };
        let mut true_w = vec![0f32; CH];
        for (c, w) in true_w.iter_mut().enumerate() {
            *w = (c as f32) * 0.01 - 0.15;
        }
        let mut xs = vec![0f32; n * CH];
        let mut ys = vec![0f32; n];
        for i in 0..n {
            let mut y = 0.3f32;
            for c in 0..CH {
                let v = rnd();
                xs[i * CH + c] = v;
                y += true_w[c] * v;
            }
            ys[i] = y;
        }
        (xs, ys, true_w)
    }

    #[test]
    fn irls_recovers_known_linear_map() {
        let n = 400usize;
        let (xs, ys, true_w) = synth(n);
        let (w, b) = DepthHead::fit_irls(&xs, n, &ys, 1e-6, 20);
        assert!((b - 0.3).abs() < 0.05, "b={b}");
        for (c, (got, want)) in w.iter().zip(true_w.iter()).enumerate() {
            assert!((got - want).abs() < 0.05, "w[{c}]={got} vs {want}");
        }
    }

    #[test]
    fn fit_beats_bias_only_on_held_out() {
        let (xs, ys, _) = synth(600);
        let (w, b) = DepthHead::fit_irls(&xs[..400 * CH], 400, &ys[..400], 1e-6, 20);
        let mut l1 = 0f64;
        let mut l1_bias = 0f64;
        for i in 400..600 {
            let mut p = f64::from(b);
            for c in 0..CH {
                p += f64::from(w[c]) * f64::from(xs[i * CH + c]);
            }
            l1 += (p - f64::from(ys[i])).abs();
            l1_bias += (f64::from(b) - f64::from(ys[i])).abs();
        }
        assert!(l1 < l1_bias * 0.1, "fit {l1} vs bias {l1_bias}");
    }

    #[test]
    fn save_load_roundtrip() {
        let mut h = DepthHead::zeros(3);
        h.w = vec![1.0, -2.0, 0.5];
        h.b = 0.25;
        h.mu = vec![0.1, 0.2, 0.3];
        h.sd = vec![1.5, 2.5, 3.5];
        let dir = std::env::temp_dir().join("tpdh_test");
        std::fs::create_dir_all(&dir).ok();
        let p = dir.join("h.tpdh");
        h.save(&p).unwrap();
        let g = DepthHead::load(&p).unwrap();
        assert_eq!(g.w, h.w);
        assert_eq!(g.b, h.b);
        assert_eq!(g.sd, h.sd);
    }
}
