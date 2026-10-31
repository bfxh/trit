//! N5 生成的第一块：可微采样器（DDIM，x0 预测）+ 小型去噪器。
//!
//! 训练：`x_t = √ᾱ_t·x₀ + √(1−ᾱ_t)·ε`，网络学 `x̂₀ = f(x_t, t, 条件)`（BCE，输出在 0..1）。
//! 采样：从纯噪声出发，按 `t = T−1 … 0` 做确定性 DDIM 更新（η=0）：
//!   `x̂₀ = f(x_t,t)`，`ε̂ = (x_t − √ᾱ_t x̂₀)/√(1−ᾱ_t)`，
//!   `x_{t−1} = √ᾱ_{t−1} x̂₀ + √(1−ᾱ_{t−1}) ε̂`。
//! 网络是**小网络**（3×3 MLP 或三值卷积栈），采样器与网络解耦，换网不用改采样。

use crate::occ::Mlp;

/// 余弦噪声表：`ᾱ_t = f(t/T)/f(0)`，长度 T+1（下标 0..=T）。
pub struct Schedule {
    pub abar: Vec<f32>,
}

impl Schedule {
    pub fn cosine(t: usize) -> Self {
        let s = 0.008f32;
        let f = |u: f32| {
            ((u + s) / (1.0 + s) * std::f32::consts::FRAC_PI_2)
                .cos()
                .powi(2)
        };
        let f0 = f(0.0);
        let abar = (0..=t)
            .map(|i| (f(i as f32 / t as f32) / f0).clamp(0.0, 1.0))
            .collect();
        Schedule { abar }
    }

    pub fn abar(&self) -> Vec<f32> {
        self.abar.clone()
    }

    /// 时间步数 T（ᾱ 有 T+1 个）。
    pub fn steps(&self) -> usize {
        self.abar.len() - 1
    }
}

/// 加噪：`x_t = √ᾱ x0 + √(1−ᾱ) ε`。
pub fn add_noise(x0: &[f32], eps: &[f32], abar_t: f32) -> Vec<f32> {
    let (s0, se) = (abar_t.sqrt(), (1.0 - abar_t).sqrt());
    x0.iter().zip(eps).map(|(a, b)| s0 * a + se * b).collect()
}

/// 去噪器接口：给定 `x_t`、时间步 t/T、条件索引 → 预测 `x0`（0..1）。
pub trait Denoiser {
    fn predict_x0(
        &self,
        xt: &[f32],
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> Vec<f32>;
    fn input_dim(&self) -> usize;
}

/// 3×3 补丁 MLP 去噪器（快，适合先把链路跑通）。
pub struct PatchMlp {
    pub mlp: Mlp,
    pub patch: usize,
    pub cond: usize,
}

impl PatchMlp {
    pub fn new(patch: usize, cond: usize, hid: usize, seed: u64) -> Self {
        let nin = patch * patch + cond + 1; // +1 = t/T
        PatchMlp {
            mlp: Mlp::new(nin, hid, seed),
            patch,
            cond,
        }
    }

    /// 特征：`xt` 的 patch + 条件 one-hot + t/T。
    #[allow(clippy::too_many_arguments)] // 渲染/训练共用的小工具，参数是它明确的上下文
    pub fn features(
        &self,
        xt: &[f32],
        x: usize,
        y: usize,
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> (Vec<f32>, f32) {
        let r = (self.patch / 2) as i32;
        let mut f = Vec::with_capacity(self.input_dim());
        for dy in -r..=r {
            for dx in -r..=r {
                let xx = ((x as i32 + dx).rem_euclid(w as i32)) as usize;
                let yy = ((y as i32 + dy).rem_euclid(h as i32)) as usize;
                f.push(xt[yy * w + xx]);
            }
        }
        for k in 0..self.cond {
            f.push((cond == Some(k)) as u8 as f32);
        }
        f.push(t_frac);
        (f, xt[y * w + x])
    }

    /// 一步训练 `k` 个像素（每个像素独立随机 t），返回平均 loss。
    #[allow(clippy::too_many_arguments)]
    pub fn train_shard(
        &mut self,
        x0: &[f32],
        abar: &[f32],
        cond: Option<usize>,
        w: usize,
        h: usize,
        lr: f32,
        k: usize,
        rng: &mut u64,
    ) -> f32 {
        let mut next = || {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            (*rng >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut sum = 0.0f64;
        for _ in 0..k {
            let t = ((next() * (abar.len() - 1) as f32) as usize).min(abar.len() - 1);
            let eps: Vec<f32> = (0..w * h)
                .map(|_| {
                    let u1 = next().max(1e-6);
                    let u2 = next();
                    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
                })
                .collect();
            let xt = add_noise(x0, &eps, abar[t]);
            let tf = t as f32 / (abar.len() - 1).max(1) as f32;
            let p = ((next() * (w * h) as f32) as usize).min(w * h - 1);
            let (f, _) = self.features(&xt, p % w, p / w, tf, cond, w, h);
            sum += f64::from(self.mlp.train_step(&f, x0[p], lr));
        }
        (sum / k.max(1) as f64) as f32
    }

    /// 单步训练：随机时间步加噪 → 回归 x0。
    #[allow(clippy::too_many_arguments)]
    pub fn train_step(
        &mut self,
        x0: &[f32],
        abar: &[f32],
        cond: Option<usize>,
        w: usize,
        h: usize,
        lr: f32,
        rng: &mut u64,
    ) -> f32 {
        let mut next = || {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            (*rng >> 11) as f32 / (1u64 << 53) as f32
        };
        let t = ((next() * (abar.len() - 1) as f32) as usize).min(abar.len() - 1);
        let eps: Vec<f32> = (0..w * h)
            .map(|_| {
                let u1 = next().max(1e-6);
                let u2 = next();
                (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
            })
            .collect();
        let xt = add_noise(x0, &eps, abar[t]);
        let tf = t as f32 / (abar.len() - 1).max(1) as f32;
        // 随机取一个像素训练（每步便宜）
        let p = ((next() * (w * h) as f32) as usize).min(w * h - 1);
        let (f, _) = self.features(&xt, p % w, p / w, tf, cond, w, h);
        self.mlp.train_step(&f, x0[p], lr)
    }
}

impl Denoiser for PatchMlp {
    fn predict_x0(
        &self,
        xt: &[f32],
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (f, _) = self.features(xt, x, y, t_frac, cond, w, h);
                let (z, _) = self.mlp.forward(&f);
                out[y * w + x] = 1.0 / (1.0 + (-z).exp());
            }
        }
        out
    }

    fn input_dim(&self) -> usize {
        self.patch * self.patch + self.cond + 1
    }
}

/// DDIM 采样（η=0）。`x_T` 为初始噪声（0..1 尺度），返回 x₀。
pub fn ddim_sample(
    d: &dyn Denoiser,
    sched: &Schedule,
    cond: Option<usize>,
    w: usize,
    h: usize,
    seed: u64,
) -> Vec<f32> {
    let abar = sched.abar();
    let t_max = abar.len() - 1;
    if t_max == 0 {
        return vec![0.0; w * h];
    }
    let mut rng = seed | 1;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f32 / (1u64 << 53) as f32
    };
    let mut x: Vec<f32> = (0..w * h)
        .map(|_| {
            let u1 = next().max(1e-6);
            let u2 = next();
            let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
            (0.5 + 0.5 * z).clamp(0.0, 1.0)
        })
        .collect();
    for t in (0..t_max).rev() {
        let at = abar[t + 1];
        let at_prev = abar[t];
        let tf = (t + 1) as f32 / t_max as f32;
        let x0 = d.predict_x0(&x, tf, cond, w, h);
        // ε̂ = (x_t − √ᾱ_t x̂₀)/√(1−ᾱ_t)
        let s0 = at.sqrt();
        let se = (1.0 - at).sqrt().max(1e-6);
        let eps: Vec<f32> = x
            .iter()
            .zip(&x0)
            .map(|(xt, x0)| (xt - s0 * x0) / se)
            .collect();
        // DDIM η=0：x_{t−1} = √ᾱ_{t−1} x̂₀ + √(1−ᾱ_{t−1}) ε̂
        let s0p = at_prev.sqrt();
        let sep = (1.0 - at_prev).sqrt();
        x = x0
            .iter()
            .zip(&eps)
            .map(|(x0, e)| (s0p * x0 + sep * e).clamp(0.0, 1.0))
            .collect();
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_is_monotone() {
        let s = Schedule::cosine(20);
        let ab = s.abar();
        assert!((ab[0] - 1.0).abs() < 1e-6);
        for i in 1..ab.len() {
            assert!(ab[i] <= ab[i - 1] + 1e-6, "ᾱ 应单调不增");
        }
        assert!(ab.last().unwrap() < &0.05);
    }

    #[test]
    fn noise_roundtrip_at_abar_one() {
        let x0 = vec![0.2f32, 0.8, 0.5];
        let eps = vec![0.3f32, -0.1, 0.0];
        let xt = add_noise(&x0, &eps, 1.0);
        assert!((xt[0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn denoiser_trains_on_constant() {
        // 常数纹理：任何 t 下模型都应学会输出 0.7
        let (w, h) = (8usize, 8usize);
        let x0 = vec![0.7f32; w * h];
        let ab = Schedule::cosine(20).abar();
        let mut d = PatchMlp::new(3, 0, 16, 5);
        let mut rng = 12345u64;
        for _ in 0..20000 {
            d.train_step(&x0, &ab, None, w, h, 0.05, &mut rng);
        }
        let pred = d.predict_x0(&vec![0.5f32; w * h], 0.5, None, w, h);
        let mean = pred.iter().sum::<f32>() / pred.len() as f32;
        assert!((mean - 0.7).abs() < 0.1, "应学到 0.7，实际 {mean:.3}");
    }
}
