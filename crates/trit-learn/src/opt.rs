//! 平面向量优化器（SGD 动量 / Adam）与梯度裁剪。
//!
//! 每个参数张量（比如 [`crate::dense::TernaryDense`] 的 latent 与 scale）
//! 各配一个优化器实例：状态按首次调用时的参数长度建立，实例之间互不共享。
//! 这是有意为之的简单约定——不用全局状态，也不做参数组。

/// 一步更新：`params -= update(grads)`。
pub trait Optimizer {
    /// 原地更新参数。`grads` 长度必须与 `params` 一致。
    fn step(&mut self, params: &mut [f32], grads: &[f32]);
}

/// 带动量的 SGD：`v = β·v + g`，`p -= lr·v`（`β = 0` 退化为普通 SGD）。
#[derive(Clone, Debug)]
pub struct Sgd {
    /// 学习率。
    pub lr: f32,
    /// 动量系数（0 = 不带动量）。
    pub momentum: f32,
    velocity: Vec<f32>,
}

impl Sgd {
    /// 新建；`momentum` 建议取 0 或 0.9。
    pub fn new(lr: f32, momentum: f32) -> Self {
        assert!(lr.is_finite() && lr > 0.0, "lr 必须是正的有限值");
        assert!(
            momentum.is_finite() && (0.0..1.0).contains(&momentum),
            "momentum 必须在 [0, 1)"
        );
        Self {
            lr,
            momentum,
            velocity: Vec::new(),
        }
    }
}

impl Optimizer for Sgd {
    fn step(&mut self, params: &mut [f32], grads: &[f32]) {
        assert_eq!(params.len(), grads.len(), "Sgd：参数与梯度长度必须一致");
        if self.momentum == 0.0 {
            for (param, &grad) in params.iter_mut().zip(grads.iter()) {
                *param -= self.lr * grad;
            }
            return;
        }
        if self.velocity.len() != params.len() {
            self.velocity = vec![0.0; params.len()];
        }
        for ((param, &grad), velocity) in params
            .iter_mut()
            .zip(grads.iter())
            .zip(self.velocity.iter_mut())
        {
            *velocity = self.momentum * *velocity + grad;
            *param -= self.lr * *velocity;
        }
    }
}

/// Adam（Kingma & Ba, 2015）：一阶/二阶矩 + 偏差修正。
#[derive(Clone, Debug)]
pub struct Adam {
    /// 学习率。
    pub lr: f32,
    /// 一阶矩衰减率。
    pub beta1: f32,
    /// 二阶矩衰减率。
    pub beta2: f32,
    /// 数值稳定项。
    pub eps: f32,
    step: u64,
    m: Vec<f32>,
    v: Vec<f32>,
}

impl Adam {
    /// 新建；默认超参与论文一致。
    pub fn new(lr: f32) -> Self {
        assert!(lr.is_finite() && lr > 0.0, "lr 必须是正的有限值");
        Self {
            lr,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            step: 0,
            m: Vec::new(),
            v: Vec::new(),
        }
    }
}

impl Optimizer for Adam {
    fn step(&mut self, params: &mut [f32], grads: &[f32]) {
        assert_eq!(params.len(), grads.len(), "Adam：参数与梯度长度必须一致");
        if self.m.len() != params.len() {
            self.m = vec![0.0; params.len()];
            self.v = vec![0.0; params.len()];
            self.step = 0;
        }
        self.step += 1;
        let bias1 = 1.0 - self.beta1.powi(self.step as i32);
        let bias2 = 1.0 - self.beta2.powi(self.step as i32);
        for (((param, &grad), m), v) in params
            .iter_mut()
            .zip(grads.iter())
            .zip(self.m.iter_mut())
            .zip(self.v.iter_mut())
        {
            *m = self.beta1 * *m + (1.0 - self.beta1) * grad;
            *v = self.beta2 * *v + (1.0 - self.beta2) * grad * grad;
            let m_hat = *m / bias1;
            let v_hat = *v / bias2;
            *param -= self.lr * m_hat / (v_hat.sqrt() + self.eps);
        }
    }
}

/// 全局范数裁剪：若 `‖grads‖₂ > max_norm`，等比缩小到 `max_norm`。
///
/// 返回裁剪前的 L2 范数（`max_norm ≤ 0` 或非有限时不裁剪）。
pub fn clip_global_norm(grads: &mut [f32], max_norm: f32) -> f32 {
    let norm = grads.iter().map(|g| g * g).sum::<f32>().sqrt();
    if !max_norm.is_finite() || max_norm <= 0.0 || norm <= max_norm {
        return norm;
    }
    let factor = max_norm / norm;
    for grad in grads.iter_mut() {
        *grad *= factor;
    }
    norm
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `f(p) = 0.5·p²`，梯度 `p`；从 `p = 4` 出发应收敛到 0。
    fn descent<O: Optimizer>(mut opt: O, steps: usize) -> f32 {
        let mut param = 4.0f32;
        for _ in 0..steps {
            let mut grad = [param];
            clip_global_norm(&mut grad, f32::INFINITY);
            opt.step(std::slice::from_mut(&mut param), &grad);
        }
        param.abs()
    }

    #[test]
    fn sgd_descends_quadratic() {
        let end = descent(Sgd::new(0.1, 0.0), 200);
        assert!(end < 0.01, "普通 SGD 收敛到 {end}");
        let end = descent(Sgd::new(0.05, 0.9), 200);
        assert!(end < 0.01, "带动量 SGD 收敛到 {end}");
    }

    #[test]
    fn adam_descends_quadratic() {
        let end = descent(Adam::new(0.1), 200);
        assert!(end < 0.01, "Adam 收敛到 {end}");
    }

    #[test]
    fn momentum_accelerates_consistent_gradient() {
        // 恒定梯度 g：第一步与普通 SGD 相同（v 从 0 起步），
        // 第二步 v = β·g + g = 1.9，位移放大 (1+β) 倍。
        let mut plain = Sgd::new(0.1, 0.0);
        let mut momentum = Sgd::new(0.1, 0.9);
        let mut a = 1.0f32;
        let mut b = 1.0f32;
        plain.step(std::slice::from_mut(&mut a), &[1.0]);
        momentum.step(std::slice::from_mut(&mut b), &[1.0]);
        assert!((a - 0.9).abs() < 1e-7);
        assert!((b - 0.9).abs() < 1e-7, "第一步 v=1，位移与普通 SGD 相同");
        plain.step(std::slice::from_mut(&mut a), &[1.0]);
        momentum.step(std::slice::from_mut(&mut b), &[1.0]);
        assert!((a - 0.8).abs() < 1e-7);
        assert!(
            (b - 0.71).abs() < 1e-7,
            "b = 0.9 - 0.1·1.9 = 0.71，实际 {b}"
        );
    }

    #[test]
    fn adam_first_step_is_lr_sized() {
        // Adam 第一步（大梯度、大 v̂）位移 ≈ lr，与梯度尺度无关。
        let mut opt = Adam::new(0.01);
        let mut param = 1.0f32;
        opt.step(std::slice::from_mut(&mut param), &[100.0]);
        assert!((param - (1.0 - 0.01)).abs() < 1e-4, "param={param}");
    }

    #[test]
    fn clip_scales_only_when_over() {
        let mut grads = [3.0, 4.0];
        let norm = clip_global_norm(&mut grads, 10.0);
        assert!((norm - 5.0).abs() < 1e-6);
        assert_eq!(grads, [3.0, 4.0], "范数未超限不应裁剪");
        let norm = clip_global_norm(&mut grads, 2.5);
        assert!((norm - 5.0).abs() < 1e-6);
        let clipped: f32 = grads.iter().map(|g| g * g).sum::<f32>().sqrt();
        assert!((clipped - 2.5).abs() < 1e-5, "裁剪后范数 {clipped}");
    }
}
