//! DDPM / DDIM 采样与分布统计。

use crate::data::Xorshift;
use crate::denoiser::Denoiser;
use crate::schedule::DiffusionSchedule;

/// 从纯噪声开始，按 DDPM 后验逐步去噪。
///
/// `n` 是并行采样的样本数（每一步整批前向，避免逐样本调用的开销）。
///
/// **数值限制（实测）**：短步数（≤200）的 cosine 调度末端 `β` 会被夹到 0.999，
/// 于是 `μ = (x - β/√(1-ᾱ)·ε̂)/√α` 里的 `1/√α ≈ 31.6`，每步把预测误差放大 31.6 倍。
/// 训练不足时采样半径会漂到 `1e3` 量级（三值与 FP 权重都会，不是三值特有）。
/// 短步数请优先用 [`ddim_sample`]（确定性、无该放大项）。
pub fn ddpm_sample<D: Denoiser + ?Sized>(
    model: &mut D,
    sched: &DiffusionSchedule,
    n: usize,
    rng: &mut Xorshift,
) -> Vec<[f32; 2]> {
    assert!(n > 0, "采样数必须非零");
    let steps = sched.steps();
    let mut x: Vec<[f32; 2]> = (0..n).map(|_| [rng.normal(), rng.normal()]).collect();
    for ti in (0..steps).rev() {
        let t = vec![ti; n];
        let pred = model.forward(&x, &t, steps);
        let ab = sched.alpha_bar(ti);
        let beta = sched.beta(ti);
        let alpha = (1.0 - beta).max(1e-12);
        let inv_sqrt_alpha = 1.0 / alpha.sqrt();
        let coef = beta / (1.0 - ab).max(1e-12).sqrt();
        let var = sched.posterior_variance(ti);
        let sd = var.sqrt();
        for i in 0..n {
            let mut p = [
                inv_sqrt_alpha * (x[i][0] - coef * pred[i][0]),
                inv_sqrt_alpha * (x[i][1] - coef * pred[i][1]),
            ];
            if ti > 0 && sd > 0.0 {
                p[0] += sd * rng.normal();
                p[1] += sd * rng.normal();
            }
            x[i] = p;
        }
    }
    x
}

/// DDIM 采样：确定性（`eta = 0`）或少步采样（`stride > 1`）。
///
/// `stride` 是跳步间隔（1 = 每步都走，等于把 DDPM 的随机项换成确定性预测）。
pub fn ddim_sample<D: Denoiser + ?Sized>(
    model: &mut D,
    sched: &DiffusionSchedule,
    n: usize,
    stride: usize,
    eta: f32,
    rng: &mut Xorshift,
) -> Vec<[f32; 2]> {
    assert!(n > 0, "采样数必须非零");
    let steps = sched.steps();
    let stride = stride.max(1);
    let mut x: Vec<[f32; 2]> = (0..n).map(|_| [rng.normal(), rng.normal()]).collect();
    let mut ti = steps - 1;
    loop {
        let t = vec![ti; n];
        let pred = model.forward(&x, &t, steps);
        let ab = sched.alpha_bar(ti).max(1e-12);
        let prev = if ti >= stride { ti - stride } else { 0 };
        let ab_prev = if ti >= stride {
            sched.alpha_bar(prev)
        } else {
            1.0
        };
        // x0 预测（夹到数据范围，避免早期步数值发散）。
        let mut x0 = vec![[0.0f32; 2]; n];
        for i in 0..n {
            x0[i][0] = ((x[i][0] - (1.0 - ab).sqrt() * pred[i][0]) / ab.sqrt()).clamp(-1.5, 1.5);
            x0[i][1] = ((x[i][1] - (1.0 - ab).sqrt() * pred[i][1]) / ab.sqrt()).clamp(-1.5, 1.5);
        }
        let sigma = eta
            * (((1.0 - ab_prev) / (1.0 - ab).max(1e-12)) * (1.0 - ab / (1.0 - ab_prev).max(1e-12)))
                .max(0.0)
                .sqrt();
        let dir = (1.0 - ab_prev - sigma * sigma).max(0.0).sqrt();
        for i in 0..n {
            let mut p = [
                ab_prev.sqrt() * x0[i][0] + dir * pred[i][0],
                ab_prev.sqrt() * x0[i][1] + dir * pred[i][1],
            ];
            if sigma > 0.0 {
                p[0] += sigma * rng.normal();
                p[1] += sigma * rng.normal();
            }
            x[i] = p;
        }
        if ti < stride {
            break;
        }
        ti = prev;
    }
    x
}

/// 采样点的分布统计（针对「环 + 盘」数据集）。
#[derive(Clone, Copy, Debug)]
pub struct RingStats {
    /// 落在半径 `1±0.2` 环带上的比例（数据里约 0.5）。
    pub ring_frac: f32,
    /// 落在半径 `<0.45` 盘内的比例（数据里约 0.5）。
    pub disk_frac: f32,
    /// 平均半径。
    pub mean_norm: f32,
    /// 半径 10% 分位。
    pub p10_r: f32,
    /// 半径 90% 分位。
    pub p90_r: f32,
}

impl RingStats {
    /// 两个模态（环/盘）是否都还在：各占 25%~75%。
    pub fn both_modes_present(&self) -> bool {
        (0.25..=0.75).contains(&self.ring_frac) && (0.25..=0.75).contains(&self.disk_frac)
    }
}

/// 统计采样点。
pub fn ring_stats(points: &[[f32; 2]]) -> RingStats {
    assert!(!points.is_empty(), "统计需要至少一个点");
    let n = points.len() as f32;
    let mut rs: Vec<f32> = points
        .iter()
        .map(|p| (p[0] * p[0] + p[1] * p[1]).sqrt())
        .collect();
    let ring = rs.iter().filter(|r| (**r - 1.0).abs() < 0.2).count() as f32;
    let disk = rs.iter().filter(|r| **r < 0.45).count() as f32;
    let mean = rs.iter().sum::<f32>() / n;
    rs.sort_by(|a, b| a.total_cmp(b));
    let pick = |f: f32| rs[((rs.len() as f32 * f) as usize).min(rs.len() - 1)];
    RingStats {
        ring_frac: ring / n,
        disk_frac: disk / n,
        mean_norm: mean,
        p10_r: pick(0.10),
        p90_r: pick(0.90),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denoiser::TernaryMlp;
    use crate::schedule::Schedule;

    #[test]
    fn ddpm_sample_is_finite() {
        let sched = DiffusionSchedule::new(Schedule::Cosine, 8);
        let mut model = TernaryMlp::new(8, 2, 0.01);
        let mut rng = Xorshift::new(9);
        let pts = ddpm_sample(&mut model, &sched, 6, &mut rng);
        assert_eq!(pts.len(), 6);
        assert!(pts.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn ddim_sample_is_finite() {
        let sched = DiffusionSchedule::linear(16);
        let mut model = TernaryMlp::new(8, 4, 0.01);
        let mut rng = Xorshift::new(13);
        let pts = ddim_sample(&mut model, &sched, 5, 4, 0.0, &mut rng);
        assert_eq!(pts.len(), 5);
        assert!(pts.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn stats_on_synthetic_modes() {
        // 手工构造 50% 环 + 50% 盘，统计应认出两模态。
        let mut pts = Vec::new();
        for i in 0..100 {
            let a = i as f32 * 0.1;
            if i % 2 == 0 {
                pts.push([a.cos(), a.sin()]);
            } else {
                pts.push([0.1 * a.cos(), 0.1 * a.sin()]);
            }
        }
        let s = ring_stats(&pts);
        assert!(s.both_modes_present(), "两模态应被识别：{s:?}");
        assert!(s.mean_norm > 0.4 && s.mean_norm < 0.7);
    }
}
