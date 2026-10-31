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

/// 无约束 argmax 解出 id 图（不依赖清单；域外判定要用原始行为）。
pub fn unconstrained_ids(logits: &[f32], hw: usize, k: usize) -> Vec<u32> {
    let ch = k * 3 + 1;
    let mut ids = vec![0u32; hw];
    for (p, slot) in ids.iter_mut().enumerate() {
        if logits[p * ch + k * 3] <= 0.0 {
            continue;
        }
        let mut id = 0u32;
        for i in 0..k {
            let b = p * ch + i * 3;
            let (a, b2, c) = (logits[b], logits[b + 1], logits[b + 2]);
            let d = if a >= b2 && a >= c {
                0
            } else if b2 >= c {
                1
            } else {
                2
            };
            id = id * 3 + d;
        }
        *slot = id + 1;
    }
    ids
}

/// 一步到特征：从 logits + `d1` 特征直接算 15 维（内部做无约束解码）。
pub fn features_from_logits(logits: &[f32], z: &[f32], hw: usize, k: usize) -> Vec<f32> {
    let ids = unconstrained_ids(logits, hw, k);
    features(&ids, logits, z, hw, k)
}

/// 阶段 2 的输入维度：15 手工特征 + `d1` 的 32 通道全局均值。
pub const NIN2: usize = NFEAT + 32;

/// 阶段 2 的特征：手工特征 + `z` 的通道均值（全局池化的线性版）。
pub fn features_v2(logits: &[f32], z: &[f32], hw: usize, k: usize) -> Vec<f32> {
    let mut f = features_from_logits(logits, z, hw, k);
    let zc = z.len() / hw;
    let inv = 1.0 / hw as f32;
    for c in 0..32 {
        let mut s = 0f32;
        if c < zc {
            for p in 0..hw {
                s += z[p * zc + c];
            }
        }
        f.push(s * inv);
    }
    f
}

/// 学习式拒识头（阶段 2）：单隐层 MLP，输入 `NIN2` 维。
#[derive(Clone, Debug)]
pub struct MlpBank {
    pub mu: Vec<f32>,
    pub sd: Vec<f32>,
    pub w1: Vec<f32>,
    pub b1: Vec<f32>,
    pub w2: Vec<f32>,
    pub b2: f32,
    pub hidden: usize,
}

impl MlpBank {
    pub fn new(hidden: usize) -> Self {
        Self {
            mu: vec![0.0; NIN2],
            sd: vec![1.0; NIN2],
            w1: vec![0.0; NIN2 * hidden],
            b1: vec![0.0; hidden],
            w2: vec![0.0; hidden],
            b2: 0.0,
            hidden,
        }
    }

    fn norm(&self, x: &[f32]) -> Vec<f32> {
        (0..NIN2)
            .map(|i| ((x[i] - self.mu[i]) / self.sd[i]).clamp(-8.0, 8.0))
            .collect()
    }

    fn forward(&self, x: &[f32]) -> (Vec<f32>, f32) {
        let h = self.hidden;
        let mut act = vec![0f32; h];
        for (j, a) in act.iter_mut().enumerate() {
            let mut acc = self.b1[j];
            for (i, xv) in x.iter().enumerate().take(NIN2) {
                acc += self.w1[i * h + j] * xv;
            }
            *a = acc.max(0.0);
        }
        let mut o = self.b2;
        for (j, a) in act.iter().enumerate() {
            o += self.w2[j] * a;
        }
        (act, o)
    }

    pub fn score(&self, x: &[f32]) -> f32 {
        let xn = self.norm(x);
        let (_, o) = self.forward(&xn);
        1.0 / (1.0 + (-o).exp())
    }

    /// 全批 Adam 训练（二元交叉熵 + L2 + 输入标准化）。
    pub fn fit(&mut self, xs: &[Vec<f32>], ys: &[f32], l2: f32, epochs: usize, seed: u64) {
        for i in 0..NIN2 {
            let n = xs.len().max(1) as f64;
            let m: f64 = xs.iter().map(|x| f64::from(x[i])).sum::<f64>() / n;
            let v: f64 = xs
                .iter()
                .map(|x| {
                    let d = f64::from(x[i]) - m;
                    d * d
                })
                .sum::<f64>()
                / n;
            self.mu[i] = m as f32;
            self.sd[i] = (v.sqrt() as f32).max(1e-4);
        }
        let h = self.hidden;
        let mut st = seed | 1;
        let mut rnd = || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st % 2001) as f32 / 1000.0 - 1.0
        };
        let a = (6.0 / NIN2 as f32).sqrt();
        for v in self.w1.iter_mut() {
            *v = rnd() * a;
        }
        for v in self.w2.iter_mut() {
            *v = rnd() * (6.0 / h as f32).sqrt();
        }
        let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
        let mut mw1 = vec![0f32; self.w1.len()];
        let mut vw1 = vec![0f32; self.w1.len()];
        let mut mb1 = vec![0f32; h];
        let mut vb1 = vec![0f32; h];
        let mut mw2 = vec![0f32; h];
        let mut vw2 = vec![0f32; h];
        let (mut mb2, mut vb2) = (0f32, 0f32);
        let lr = 3e-3f32;
        let n = xs.len().max(1) as f32;
        for ep in 0..epochs {
            let bc1 = 1.0 - b1.powf(ep as f32 + 1.0);
            let bc2 = 1.0 - b2.powf(ep as f32 + 1.0);
            let mut gw1 = vec![0f32; self.w1.len()];
            let mut gb1 = vec![0f32; h];
            let mut gw2 = vec![0f32; h];
            let mut gb2 = 0f32;
            for (x, &y) in xs.iter().zip(ys) {
                let xn = self.norm(x);
                let (act, o) = self.forward(&xn);
                let p = 1.0 / (1.0 + (-o).exp());
                let d = (p - y) / n;
                for j in 0..h {
                    gw2[j] += d * act[j];
                    if act[j] > 0.0 {
                        let dj = d * self.w2[j];
                        gb1[j] += dj;
                        for i in 0..NIN2 {
                            gw1[i * h + j] += dj * xn[i];
                        }
                    }
                }
                gb2 += d;
            }
            let step = |p: &mut f32, g: f32, m: &mut f32, v: &mut f32, wd: f32| {
                *m = b1 * *m + (1.0 - b1) * g;
                *v = b2 * *v + (1.0 - b2) * g * g;
                *p -= lr * ((*m / bc1) / ((*v / bc2).sqrt() + eps) + wd * *p);
            };
            for i in 0..self.w1.len() {
                step(&mut self.w1[i], gw1[i], &mut mw1[i], &mut vw1[i], l2);
            }
            for j in 0..h {
                step(&mut self.b1[j], gb1[j], &mut mb1[j], &mut vb1[j], 0.0);
                step(&mut self.w2[j], gw2[j], &mut mw2[j], &mut vw2[j], l2);
            }
            step(&mut self.b2, gb2, &mut mb2, &mut vb2, 0.0);
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::fmt::Write as _;
        let mut s = String::from(
            "TPO2
",
        );
        let _ = writeln!(s, "{} {} {}", self.hidden, self.b2, NIN2);
        for i in 0..NIN2 {
            let _ = writeln!(s, "{} {}", self.mu[i], self.sd[i]);
        }
        for j in 0..self.hidden {
            let _ = writeln!(s, "{} {}", self.w2[j], self.b1[j]);
        }
        for i in 0..NIN2 {
            for j in 0..self.hidden {
                let _ = write!(s, "{} ", self.w1[i * self.hidden + j]);
            }
            let _ = writeln!(s);
        }
        std::fs::write(path, s)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut it = s.lines();
        if it.next() != Some("TPO2") {
            return Err("不是 TPO2".into());
        }
        let head = it.next().ok_or("缺头")?;
        let v: Vec<&str> = head.split_whitespace().collect();
        let hidden: usize = v.first().and_then(|x| x.parse().ok()).ok_or("缺 hidden")?;
        let b2: f32 = v.get(1).and_then(|x| x.parse().ok()).ok_or("缺 b2")?;
        let mut m = MlpBank::new(hidden);
        m.b2 = b2;
        for i in 0..NIN2 {
            let l = it.next().ok_or("行数不足（标准化）")?;
            let p: Vec<f32> = l
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            m.mu[i] = p[0];
            m.sd[i] = p[1];
        }
        for j in 0..hidden {
            let l = it.next().ok_or("行数不足（隐层）")?;
            let p: Vec<f32> = l
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            m.w2[j] = p[0];
            m.b1[j] = p[1];
        }
        for j in 0..NIN2 {
            let l = it.next().ok_or("行数不足（w1）")?;
            let p: Vec<f32> = l
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            for (k, v) in p.iter().enumerate().take(hidden) {
                m.w1[j * hidden + k] = *v;
            }
        }
        Ok(m)
    }
}

/// 标准化 + 逻辑回归（L2）。/// 标准化 + 逻辑回归（L2）。/// 标准化 + 逻辑回归（L2）。
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

/// 闸门银行：线性（TPOD1）或 MLP（TPO2），按文件首行自动识别。
pub enum Bank {
    Lr(OodBank),
    Mlp(MlpBank),
}

impl Bank {
    pub fn load(path: &Path) -> Result<Self, String> {
        let head = std::fs::read_to_string(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        if head == "TPO2" {
            Ok(Bank::Mlp(MlpBank::load(path)?))
        } else {
            Ok(Bank::Lr(OodBank::load(path)?))
        }
    }

    pub fn score(&self, x: &[f32]) -> f32 {
        match self {
            Bank::Lr(b) => b.score(x),
            Bank::Mlp(m) => m.score(x),
        }
    }

    /// MLP 需要 47 维输入（15 手工 + 32 个 z 通道均值）；线性门只用 15 维。
    pub fn needs_v2(&self) -> bool {
        matches!(self, Bank::Mlp(_))
    }
}

/// ROC-AUC（`scores` 越正越域内）。/// ROC-AUC（`scores` 越正越域内）。
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
