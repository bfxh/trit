//! DDPM 训练循环：随机时间步、预测噪声、MSE 反传。
//!
//! 每一步都重新采样 `x₀` 批与 `t`，噪声 `ε ~ N(0,1)`。损失是 `‖ε̂ - ε‖²` 的均值，
//! 梯度经三值层的 STE 直通回 latent 权重。

use std::time::Instant;

use trit_learn::loss::mse;

use crate::data::Xorshift;
use crate::denoiser::Denoiser;
use crate::schedule::DiffusionSchedule;

/// 训练超参数。
#[derive(Clone, Copy, Debug)]
pub struct TrainOptions {
    /// 训练步数（不是 epoch）。
    pub steps: usize,
    /// 批大小。
    pub batch: usize,
    /// 随机种子（同时决定批采样与噪声）。
    pub seed: u64,
    /// 每多少步记录一次损失（0 = 只记首尾）。
    pub log_every: usize,
}

impl Default for TrainOptions {
    fn default() -> Self {
        Self {
            steps: 1500,
            batch: 64,
            seed: 42,
            log_every: 250,
        }
    }
}

/// 训练结果。
#[derive(Clone, Debug)]
pub struct TrainReport {
    /// 第 1 步的损失。
    pub initial_loss: f32,
    /// 最后一步的损失。
    pub final_loss: f32,
    /// 损失曲线（按 `log_every` 抽点，末尾必含最后一步）。
    pub loss_curve: Vec<f32>,
    /// 训练耗时（秒）。
    pub secs: f32,
    /// 权重理论存储位数。
    pub weight_bits: usize,
}

/// 训练一个去噪器。
pub fn train<D: Denoiser + ?Sized>(
    model: &mut D,
    sched: &DiffusionSchedule,
    data: &[[f32; 2]],
    opts: &TrainOptions,
) -> TrainReport {
    assert!(!data.is_empty(), "训练数据不能为空");
    assert!(opts.steps > 0, "训练步数必须至少 1");
    assert!(opts.batch > 0, "批大小必须非零");
    let t_max = sched.steps();
    let mut rng = Xorshift::new(opts.seed);
    let mut curve = Vec::new();
    let start = Instant::now();
    let mut first = f32::NAN;
    let mut last = f32::NAN;

    for step in 0..opts.steps {
        let b = opts.batch;
        // 1. 采样 x0 批。
        let mut x0 = Vec::with_capacity(b);
        for _ in 0..b {
            let idx = (rng.next_u64() as usize) % data.len();
            x0.push(data[idx]);
        }
        // 2. 每个样本独立时间步（DDPM 的标准做法：t 均匀采样）。
        let mut t = Vec::with_capacity(b);
        for _ in 0..b {
            t.push((rng.next_u64() as usize) % t_max);
        }
        // 3. 标准正态噪声。
        let mut eps = Vec::with_capacity(b);
        for _ in 0..b {
            eps.push([rng.normal(), rng.normal()]);
        }
        // 4. 前向加噪。
        let mut xt = Vec::with_capacity(b);
        for i in 0..b {
            let ab = sched.alpha_bar(t[i]);
            let s0 = ab.sqrt();
            let s1 = (1.0 - ab).max(0.0).sqrt();
            xt.push([
                s0 * x0[i][0] + s1 * eps[i][0],
                s0 * x0[i][1] + s1 * eps[i][1],
            ]);
        }
        // 5. 预测噪声 + MSE。
        let pred = model.forward(&xt, &t, t_max);
        let mut pf = Vec::with_capacity(b * 2);
        let mut ef = Vec::with_capacity(b * 2);
        for i in 0..b {
            pf.push(pred[i][0]);
            pf.push(pred[i][1]);
            ef.push(eps[i][0]);
            ef.push(eps[i][1]);
        }
        let (loss, grad_flat) = mse(&pf, &ef);
        assert!(loss.is_finite(), "第 {step} 步损失不是有限值");
        // 6. 反传 + 更新。
        let mut grad = Vec::with_capacity(b);
        for i in 0..b {
            grad.push([grad_flat[2 * i], grad_flat[2 * i + 1]]);
        }
        model.zero_grad();
        model.backward(&grad);
        model.update();

        if step == 0 {
            first = loss;
        }
        last = loss;
        if opts.log_every > 0 && step % opts.log_every == 0 {
            curve.push(loss);
        }
    }
    curve.push(last);

    TrainReport {
        initial_loss: first,
        final_loss: last,
        loss_curve: curve,
        secs: start.elapsed().as_secs_f32(),
        weight_bits: model.weight_bits(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::sample_ring;
    use crate::denoiser::TernaryMlp;
    use crate::schedule::Schedule;

    #[test]
    fn training_reduces_loss() {
        // 小网络、短训练：只验证「能学」——损失必须显著下降。
        let sched = DiffusionSchedule::new(Schedule::Cosine, 16);
        let mut rng = Xorshift::new(5);
        let data = sample_ring(256, &mut rng);
        let mut model = TernaryMlp::new(16, 7, 0.02);
        let report = train(
            &mut model,
            &sched,
            &data,
            &TrainOptions {
                steps: 120,
                batch: 32,
                seed: 3,
                log_every: 20,
            },
        );
        assert!(report.initial_loss.is_finite() && report.final_loss.is_finite());
        assert!(
            report.final_loss < report.initial_loss * 0.9,
            "损失未下降：{} → {}",
            report.initial_loss,
            report.final_loss
        );
        assert_eq!(report.weight_bits, model.weight_bits());
    }

    #[test]
    fn rejects_empty_data() {
        let sched = DiffusionSchedule::linear(8);
        let mut model = TernaryMlp::new(4, 1, 0.01);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            train(&mut model, &sched, &[], &TrainOptions::default())
        }));
        assert!(r.is_err(), "空数据必须 panic（fail-closed）");
    }
}
