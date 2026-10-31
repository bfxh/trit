//! 域外拒识（阶段 1）：手工特征 + 逻辑回归，纯主机侧，不改网络。
//!
//! 特征全部来自一次前向：预测 id 图、head logits、`d1` 特征 `z`。
//! 判定「这张图是不是训练过的装配体」；分割结果完全不受影响（旁路）。

use std::path::Path;

/// 特征个数。
pub const NFEAT: usize = 15;

/// 从一次前向的产物里抽特征（顺序固定，改顺序要让旧模型失效）。
pub fn features(ids: &[u32], logits: &[f32], z: &[f32], hw: usize, k: usize) -> Vec<f32> {
    let ch = k * 3 + 1;
    assert_eq!(logits.len(), hw * ch);
    let zc = z.len() / hw;

    let part = ids.iter().filter(|&&v| v > 0).count() as f32 / hw as f32;

    let mut vsum = 0f64;
    let mut vsum2 = 0f64;
    for p in 0..hw {
        let v = logits[p * ch + k * 3];
        vsum += f64::from(v);
        vsum2 += f64::from(v) * f64::from(v);
    }
    let vmean = (vsum / hw as f64) as f32;
    let vvar = (vsum2 / hw as f64 - f64::from(vmean) * f64::from(vmean)).max(0.0);
    let vstd = vvar.sqrt() as f32;

    // valid 判定的二元熵（用 sigmoid 的均值熵近似，稳定且便宜）
    let mut hent = 0f64;
    for p in 0..hw {
        let v = logits[p * ch + k * 3];
        let s = 1.0 / (1.0 + (-v).exp());
        let s = f64::from(s.clamp(1e-6, 1.0 - 1e-6));
        hent -= s * s.ln() + (1.0 - s) * (1.0 - s).ln();
    }
    let hent = (hent / hw as f64) as f32;

    // id 图邻域一致率（域内高、噪声低）
    let mut same = 0usize;
    let mut both = 0usize;
    let w = (hw as f64).sqrt() as usize;
    for y in 0..w {
        for x in 0..w {
            let i = y * w + x;
            if ids[i] == 0 {
                continue;
            }
            if x + 1 < w && ids[i + 1] > 0 {
                both += 1;
                if ids[i] == ids[i + 1] {
                    same += 1;
                }
            }
            if y + 1 < w && ids[i + w] > 0 {
                both += 1;
                if ids[i] == ids[i + w] {
                    same += 1;
                }
            }
        }
    }
    let agree = same as f32 / both.max(1) as f32;

    // 每平面 softmax 置信度均值
    let mut conf = vec![0f64; k];
    for p in 0..hw {
        for (i, slot) in conf.iter_mut().enumerate() {
            let b = p * ch + i * 3;
            let (a, b2, c) = (logits[b], logits[b + 1], logits[b + 2]);
            let m = a.max(b2).max(c);
            let e = [(a - m).exp(), (b2 - m).exp(), (c - m).exp()];
            let s = e[0] + e[1] + e[2];
            *slot += f64::from(e.iter().cloned().fold(0.0f32, f32::max) / s);
        }
    }

    // z 的幅值：总体 + 4 组均值
    let mut zabs = 0f64;
    let mut groups = [0f64; 4];
    for p in 0..hw {
        for c in 0..zc {
            let a = f64::from(z[p * zc + c].abs());
            zabs += a;
            groups[(c * 4 / zc.max(1)).min(3)] += a;
        }
    }
    let zabs = (zabs / (hw * zc) as f64) as f32;
    let gscale = 1.0 / (hw * (zc / 4).max(1)) as f64;

    let mut f = vec![part, vmean, vstd, hent, agree, zabs];
    for v in &conf {
        f.push((v / hw as f64) as f32);
    }
    for g in groups {
        f.push((g * gscale) as f32);
    }
    debug_assert_eq!(f.len(), NFEAT);
    f
}

/// 标准化 + 逻辑回归（L2）。
#[derive(Clone, Debug)]
pub struct OodBank {
    pub mu: Vec<f32>,
    pub sd: Vec<f32>,
    pub w: Vec<f32>,
    pub b: f32,
}

impl OodBank {
    pub fn zeros() -> Self {
        Self {
            mu: vec![0.0; NFEAT],
            sd: vec![1.0; NFEAT],
            w: vec![0.0; NFEAT],
            b: 0.0,
        }
    }

    pub fn calibrate(&mut self, xs: &[Vec<f32>]) {
        for c in 0..NFEAT {
            let n = xs.len().max(1) as f64;
            let m: f64 = xs.iter().map(|x| f64::from(x[c])).sum::<f64>() / n;
            let v: f64 = xs
                .iter()
                .map(|x| {
                    let d = f64::from(x[c]) - m;
                    d * d
                })
                .sum::<f64>()
                / n;
            self.mu[c] = m as f32;
            self.sd[c] = (v.sqrt() as f32).max(1e-4);
        }
    }

    /// 拟合：`xs` 与 `ys`（1=域内）。全批梯度下降 + L2。
    pub fn fit(&mut self, xs: &[Vec<f32>], ys: &[f32], l2: f32, epochs: usize) {
        self.calibrate(xs);
        let n = xs.len().max(1) as f32;
        let mut lr = 0.5f32;
        for _ in 0..epochs {
            let mut gw = [0f32; NFEAT];
            let mut gb = 0f32;
            for (x, &y) in xs.iter().zip(ys) {
                let s = self.score_raw(x);
                let err = s - y;
                for (c, g) in gw.iter_mut().enumerate() {
                    *g += err * (x[c] - self.mu[c]) / self.sd[c];
                }
                gb += err;
            }
            for (c, ws) in self.w.iter_mut().enumerate() {
                *ws -= lr * (gw[c] / n + l2 * *ws);
            }
            self.b -= lr * (gb / n);
            lr *= 0.999;
        }
    }

    fn score_raw(&self, x: &[f32]) -> f32 {
        let mut acc = self.b;
        for (c, w) in self.w.iter().enumerate() {
            acc += w * (x[c] - self.mu[c]) / self.sd[c];
        }
        1.0 / (1.0 + (-acc).exp())
    }

    /// 域内概率（0..1）。
    pub fn score(&self, x: &[f32]) -> f32 {
        self.score_raw(x)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut s = String::from("TPOD1\n");
        s.push_str(&format!("{}\n", self.b));
        for c in 0..NFEAT {
            s.push_str(&format!(
                "{} {} {} {}\n",
                self.w[c], self.mu[c], self.sd[c], c
            ));
        }
        std::fs::write(path, s)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut it = s.lines();
        if it.next() != Some("TPOD1") {
            return Err("不是 TPOD1".into());
        }
        let b: f32 = it.next().and_then(|v| v.parse().ok()).ok_or("缺 bias")?;
        let mut bank = OodBank::zeros();
        bank.b = b;
        for c in 0..NFEAT {
            let line = it.next().ok_or("行数不足")?;
            let v: Vec<f32> = line
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if v.len() < 3 {
                return Err("列数不足".into());
            }
            bank.w[c] = v[0];
            bank.mu[c] = v[1];
            bank.sd[c] = v[2];
        }
        Ok(bank)
    }
}

/// ROC-AUC（`scores` 越正越域内）。
pub fn auc(scores: &[f32], pos: &[bool]) -> f32 {
    let ps: Vec<f32> = scores
        .iter()
        .zip(pos)
        .filter(|(_, p)| **p)
        .map(|(&s, _)| s)
        .collect();
    let ns: Vec<f32> = scores
        .iter()
        .zip(pos)
        .filter(|(_, p)| !**p)
        .map(|(&s, _)| s)
        .collect();
    if ps.is_empty() || ns.is_empty() {
        return f32::NAN;
    }
    let mut acc = 0f64;
    for &a in &ps {
        for &b in &ns {
            acc += if a > b {
                1.0
            } else if (a - b).abs() < 1e-9 {
                0.5
            } else {
                0.0
            };
        }
    }
    (acc / (ps.len() * ns.len()) as f64) as f32
}

/// 在给定 FPR 上限下能达到的 TPR（阈值在给定集合上取最优，用于 dev 上定阈值）。
pub fn tpr_at_fpr(scores: &[f32], pos: &[bool], max_fpr: f32) -> (f32, f32) {
    let mut ths: Vec<f32> = scores.to_vec();
    ths.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut best = (0f32, f32::NEG_INFINITY);
    for &t in &ths {
        let tp = scores
            .iter()
            .zip(pos)
            .filter(|(s, p)| **p && **s >= t)
            .count() as f32;
        let fp = scores
            .iter()
            .zip(pos)
            .filter(|(s, p)| !**p && **s >= t)
            .count() as f32;
        let n_pos = pos.iter().filter(|&&p| p).count() as f32;
        let n_neg = pos.len() as f32 - n_pos;
        let fpr = fp / n_neg.max(1.0);
        if fpr <= max_fpr {
            let tpr = tp / n_pos.max(1.0);
            if tpr > best.0 {
                best = (tpr, t);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(n: usize, sep: f32, seed: u64) -> (Vec<Vec<f32>>, Vec<f32>) {
        let mut st = seed | 1;
        let mut r = || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st % 2001) as f32 / 1000.0 - 1.0
        };
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for i in 0..n {
            let pos = i % 2 == 0;
            let mut x = vec![0f32; NFEAT];
            for v in x.iter_mut() {
                *v = r();
            }
            x[0] = if pos { sep } else { -sep } + r() * 0.3;
            xs.push(x);
            ys.push(f32::from(u8::from(pos)));
        }
        (xs, ys)
    }

    #[test]
    fn fit_separates_two_gaussians() {
        let (xs, ys) = synth(200, 1.5, 7);
        let mut bank = OodBank::zeros();
        bank.fit(&xs, &ys, 1e-3, 800);
        let scores: Vec<f32> = xs.iter().map(|x| bank.score(x)).collect();
        let pos: Vec<bool> = ys.iter().map(|&v| v > 0.5).collect();
        assert!(auc(&scores, &pos) > 0.95, "auc={}", auc(&scores, &pos));
    }

    #[test]
    fn auc_is_half_for_random_scores() {
        let scores = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let pos = vec![true, false, true, false, true, false];
        let a = auc(&scores, &pos);
        assert!((a - 0.5).abs() < 0.3, "auc={a}");
    }

    #[test]
    fn save_load_roundtrip() {
        let mut b = OodBank::zeros();
        b.w[0] = 1.5;
        b.b = -0.25;
        let p = std::env::temp_dir().join("ood_bank_test.tpod");
        b.save(&p).unwrap();
        let c = OodBank::load(&p).unwrap();
        assert_eq!(c.w, b.w);
        assert_eq!(c.b, b.b);
    }
}
