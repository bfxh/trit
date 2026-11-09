//! β / α / ᾱ 调度：DDPM 前向加噪与反向采样共用的系数表。
//!
//! 约定与论文一致：`t = 0` 是**最轻**噪声，`t = steps - 1` 是**最重**噪声；
//! `alpha_bar[t] = Π_{i<=t}(1 - beta_i)` 单调递减。

/// 调度类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schedule {
    /// 线性 β：`1e-4 → 0.02`（Ho et al. 2020 的默认）。
    Linear,
    /// 余弦 ᾱ（Nichol & Dhariwal 2021）：信噪比在整条轨迹上更均匀。
    Cosine,
}

/// 预计算的调度表。
#[derive(Clone, Debug)]
pub struct DiffusionSchedule {
    betas: Vec<f32>,
    alpha_bars: Vec<f32>,
}

impl DiffusionSchedule {
    /// 按类型与步数构造。
    pub fn new(kind: Schedule, steps: usize) -> Self {
        assert!(steps > 0, "扩散步数必须非零");
        match kind {
            Schedule::Linear => Self::linear(steps),
            Schedule::Cosine => Self::cosine(steps),
        }
    }

    /// 线性 β 调度。
    pub fn linear(steps: usize) -> Self {
        assert!(steps > 0, "扩散步数必须非零");
        let (lo, hi) = (1e-4f32, 0.02f32);
        let betas: Vec<f32> = (0..steps)
            .map(|i| {
                if steps == 1 {
                    lo
                } else {
                    lo + (hi - lo) * i as f32 / (steps - 1) as f32
                }
            })
            .collect();
        Self::from_betas(betas)
    }

    /// 余弦 ᾱ 调度：`ᾱ_t = f(t+1)/f(0)`，`f(t) = cos²((t/T + s)/(1 + s) · π/2)`。
    pub fn cosine(steps: usize) -> Self {
        assert!(steps > 0, "扩散步数必须非零");
        let s = 0.008f32;
        let f = |t: usize| {
            let x = ((t as f32 / steps as f32 + s) / (1.0 + s)) * std::f32::consts::FRAC_PI_2;
            let c = (x as f64).cos() as f32;
            c * c
        };
        let f0 = f(0).max(1e-12);
        let mut betas = Vec::with_capacity(steps);
        let mut prev = 1.0f32;
        for t in 0..steps {
            let ab = (f(t + 1) / f0).clamp(1e-9, 1.0);
            let beta = (1.0 - ab / prev).clamp(1e-7, 0.999);
            betas.push(beta);
            prev = ab;
        }
        Self::from_betas(betas)
    }

    fn from_betas(betas: Vec<f32>) -> Self {
        let mut alpha_bars = Vec::with_capacity(betas.len());
        let mut acc = 1.0f32;
        for &b in &betas {
            acc *= 1.0 - b;
            alpha_bars.push(acc);
        }
        Self {
            betas,
            alpha_bars,
        }
    }

    /// 总步数 `T`。
    pub fn steps(&self) -> usize {
        self.betas.len()
    }

    /// `β_t`。
    pub fn beta(&self, t: usize) -> f32 {
        self.betas[t.min(self.betas.len() - 1)]
    }

    /// `ᾱ_t`。
    pub fn alpha_bar(&self, t: usize) -> f32 {
        self.alpha_bars[t.min(self.alpha_bars.len() - 1)]
    }

    /// 前向加噪：`x_t = √ᾱ_t · x₀ + √(1-ᾱ_t) · ε`。
    pub fn forward(&self, x0: &[f32], t: usize, eps: &[f32]) -> Vec<f32> {
        assert_eq!(x0.len(), eps.len(), "x0 与 eps 长度必须一致");
        let ab = self.alpha_bar(t);
        let s0 = ab.sqrt();
        let s1 = (1.0 - ab).max(0.0).sqrt();
        x0.iter()
            .zip(eps)
            .map(|(&x, &e)| s0 * x + s1 * e)
            .collect()
    }

    /// 后验方差 `β̃_t = β_t · (1-ᾱ_{t-1}) / (1-ᾱ_t)`（`t = 0` 时为 0）。
    pub fn posterior_variance(&self, t: usize) -> f32 {
        if t == 0 {
            return 0.0;
        }
        let beta = self.beta(t);
        let ab = self.alpha_bar(t);
        let ab_prev = self.alpha_bar(t - 1);
        (beta * (1.0 - ab_prev) / (1.0 - ab).max(1e-12)).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_alpha_bars_decrease() {
        // 线性调度要到千步量级才接近纯噪声（Ho et al. 用 T=1000）。
        let s = DiffusionSchedule::linear(1000);
        assert_eq!(s.steps(), 1000);
        for t in 1..s.steps() {
            assert!(
                s.alpha_bar(t) < s.alpha_bar(t - 1),
                "ᾱ 必须严格递减：t={t}"
            );
        }
        assert!(s.alpha_bar(0) < 1.0 && s.alpha_bar(0) > 0.999);
        assert!(s.alpha_bar(999) < 0.01, "千步线性末端应接近纯噪声");
    }

    #[test]
    fn short_linear_keeps_some_signal() {
        // 200 步线性末端 ᾱ ≈ 0.13：这是线性调度的真实性质，不能被误当作 bug。
        let s = DiffusionSchedule::linear(200);
        let last = s.alpha_bar(199);
        assert!(
            (0.05..0.25).contains(&last),
            "200 步线性末端 ᾱ 应在 0.05~0.25：{last}"
        );
    }

    #[test]
    fn cosine_alpha_bars_decrease() {
        let s = DiffusionSchedule::cosine(300);
        for t in 1..s.steps() {
            assert!(s.alpha_bar(t) < s.alpha_bar(t - 1), "余弦 ᾱ 必须递减：t={t}");
        }
        assert!(s.beta(0) > 0.0 && s.beta(0) < 0.05);
    }

    #[test]
    fn forward_endpoints() {
        // 余弦调度两端才能真正到 0/1（线性 200 步末端 ᾱ 还有 0.13）。
        let s = DiffusionSchedule::cosine(100);
        let x0 = [0.5f32, -0.25];
        let eps = [1.0f32, -1.0];
        // 最轻噪声：接近 x0
        let x0_light = s.forward(&x0, 0, &eps);
        assert!((x0_light[0] - x0[0]).abs() < 0.06);
        // 最重噪声：接近 eps
        let x_heavy = s.forward(&x0, 99, &eps);
        assert!((x_heavy[0] - eps[0]).abs() < 0.05);
    }

    #[test]
    fn posterior_variance_non_negative() {
        let s = DiffusionSchedule::cosine(64);
        assert_eq!(s.posterior_variance(0), 0.0);
        for t in 1..s.steps() {
            let v = s.posterior_variance(t);
            assert!(v >= 0.0 && v.is_finite(), "后验方差非法：t={t} v={v}");
        }
    }
}
