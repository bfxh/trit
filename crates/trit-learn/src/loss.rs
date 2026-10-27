//! 损失函数与解析梯度。
//!
//! 每个函数返回 `(loss, dL/dy)`，梯度是逐元素的；形状不符直接 panic
//! （与 [`crate::dense`] 的批量约定一致，入口处就该被拦下）。

/// 逐元素 `0.5·(y-t)²` 的均值。
///
/// 返回 `(loss, dL/dy)`，其中 `dL/dy[i] = (y[i]-t[i]) / n`。
pub fn mse(y: &[f32], target: &[f32]) -> (f32, Vec<f32>) {
    assert_eq!(y.len(), target.len(), "mse：预测与目标长度必须一致");
    let n = y.len().max(1) as f32;
    let mut loss = 0.0f32;
    let mut grad = Vec::with_capacity(y.len());
    for (&yi, &ti) in y.iter().zip(target.iter()) {
        let diff = yi - ti;
        loss += 0.5 * diff * diff;
        grad.push(diff / n);
    }
    (loss / n, grad)
}

/// 二分类 hinge（标签 `±1`）：逐元素 `max(0, 1 - t·y)` 的均值。
///
/// 边距内（`t·y < 1`）梯度是 `-t/n`，边距外梯度为 0。
pub fn hinge(y: &[f32], target: &[f32]) -> (f32, Vec<f32>) {
    assert_eq!(y.len(), target.len(), "hinge：预测与目标长度必须一致");
    let n = y.len().max(1) as f32;
    let mut loss = 0.0f32;
    let mut grad = Vec::with_capacity(y.len());
    for (&yi, &ti) in y.iter().zip(target.iter()) {
        let margin = 1.0 - ti * yi;
        if margin > 0.0 {
            loss += margin;
            grad.push(-ti / n);
        } else {
            grad.push(0.0);
        }
    }
    (loss / n, grad)
}

/// softmax（数值稳定：先减行最大值）。
///
/// 指数在 **f64 内部求值再落回 f32**：各平台 libm 的 f32 `exp` 相差可达 1 ulp，
/// 多轮训练会放大成不同轨迹；f64 内部计算是训练读数「可复算」的前提
/// （与 [`crate::activation`] 的约定一致）。
pub fn softmax(logits: &[f32]) -> Vec<f32> {
    assert!(!logits.is_empty(), "softmax：logits 不能为空");
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f64> = logits.iter().map(|&z| ((z - max) as f64).exp()).collect();
    let sum: f64 = exps.iter().sum();
    exps.into_iter().map(|e| (e / sum) as f32).collect()
}

/// softmax 交叉熵：`loss = -ln(p[label])`，`dL/dz = p - onehot(label)`。
pub fn cross_entropy(logits: &[f32], label: usize) -> (f32, Vec<f32>) {
    let p = softmax(logits);
    let prob = p
        .get(label)
        .copied()
        .unwrap_or_else(|| panic!("cross_entropy：label {label} 越界（共 {} 类）", p.len()));
    let loss = -((prob.max(f32::MIN_POSITIVE)) as f64).ln() as f32;
    let grad = p
        .iter()
        .enumerate()
        .map(|(index, &pi)| if index == label { pi - 1.0 } else { pi })
        .collect();
    (loss, grad)
}

/// softmax 的向量-Jacobian 积：`dL/dz = p ⊙ (g - (g·p))`，其中 `p = softmax(z)`、
/// `g = dL/dp`。注意力反向（对 logits 的梯度）直接用它。
pub fn softmax_backward(prob: &[f32], grad_prob: &[f32]) -> Vec<f32> {
    assert_eq!(
        prob.len(),
        grad_prob.len(),
        "softmax_backward：概率与上游梯度长度必须一致"
    );
    let dot: f32 = prob
        .iter()
        .zip(grad_prob.iter())
        .map(|(&p, &g)| p * g)
        .sum();
    prob.iter()
        .zip(grad_prob.iter())
        .map(|(&p, &g)| p * (g - dot))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn f32(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 % 2000) as f32) / 1000.0 - 1.0
        }
    }

    #[test]
    fn mse_value_and_gradient() {
        let (loss, grad) = mse(&[1.0, 3.0], &[0.0, 2.0]);
        // 0.5·(1²+1²)/2 = 0.5
        assert!((loss - 0.5).abs() < 1e-7);
        assert!((grad[0] - 0.5).abs() < 1e-7);
        assert!((grad[1] - 0.5).abs() < 1e-7);
    }

    #[test]
    fn mse_gradient_matches_numeric() {
        let mut rng = Rng(0xBEEF);
        let target: Vec<f32> = (0..6).map(|_| rng.f32()).collect();
        let y: Vec<f32> = (0..6).map(|_| rng.f32()).collect();
        let (_, grad) = mse(&y, &target);
        let h = 1e-3;
        for index in 0..y.len() {
            let mut up = y.clone();
            up[index] += h;
            let mut down = y.clone();
            down[index] -= h;
            let numeric = (mse(&up, &target).0 - mse(&down, &target).0) / (2.0 * h);
            assert!(
                (grad[index] - numeric).abs() < 1e-4,
                "mse 梯度[{index}]：解析 {} vs 数值 {numeric}",
                grad[index]
            );
        }
    }

    #[test]
    fn hinge_margin_and_gradient() {
        // 边距内：loss = 1 - 0.5·1 = 0.5，梯度 -1/2。
        let (loss, grad) = hinge(&[0.5, 2.0], &[1.0, 1.0]);
        assert!((loss - 0.25).abs() < 1e-7, "loss={loss}");
        assert!((grad[0] + 0.5).abs() < 1e-7);
        assert_eq!(grad[1], 0.0);
    }

    #[test]
    fn hinge_gradient_matches_numeric() {
        let target = [1.0, -1.0, 1.0];
        let y = [0.3, -0.4, 1.5];
        let (_, grad) = hinge(&y, &target);
        let h = 1e-3;
        for index in 0..3 {
            let mut up = y;
            up[index] += h;
            let mut down = y;
            down[index] -= h;
            let numeric = (hinge(&up, &target).0 - hinge(&down, &target).0) / (2.0 * h);
            assert!((grad[index] - numeric).abs() < 1e-4);
        }
    }

    #[test]
    fn softmax_sums_to_one_and_is_stable() {
        let p = softmax(&[1.0, 2.0, 3.0]);
        let sum: f32 = p.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        assert!(p[2] > p[1] && p[1] > p[0]);
        // 大数不溢出：平移不变性。
        let shifted = softmax(&[1001.0, 1002.0, 1003.0]);
        for (a, b) in p.iter().zip(shifted.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn cross_entropy_gradient_matches_numeric() {
        let mut rng = Rng(0xCAFE);
        let logits: Vec<f32> = (0..5).map(|_| rng.f32()).collect();
        for label in 0..5 {
            let (loss, grad) = cross_entropy(&logits, label);
            assert!(loss.is_finite() && loss > 0.0);
            // h=1e-3：f32 中心差分的舍入误差 ~ eps·|L|/h，取太小反而更不准。
            let h = 1e-3;
            for index in 0..logits.len() {
                let mut up = logits.clone();
                up[index] += h;
                let mut down = logits.clone();
                down[index] -= h;
                let numeric =
                    (cross_entropy(&up, label).0 - cross_entropy(&down, label).0) / (2.0 * h);
                assert!(
                    (grad[index] - numeric).abs() < 1e-3,
                    "label={label} 梯度[{index}]：解析 {} vs 数值 {numeric}",
                    grad[index]
                );
            }
        }
    }

    #[test]
    fn softmax_backward_matches_numeric() {
        // L(z) = Σ g_i·softmax(z)_i，对 z 的导数应等于 softmax_backward(p, g)。
        let mut rng = Rng(0x5A17);
        let logits: Vec<f32> = (0..6).map(|_| rng.f32()).collect();
        let g: Vec<f32> = (0..6).map(|_| rng.f32()).collect();
        let p = softmax(&logits);
        let grad = softmax_backward(&p, &g);
        let loss =
            |z: &[f32]| -> f32 { softmax(z).iter().zip(g.iter()).map(|(&p, &g)| p * g).sum() };
        let h = 1e-3;
        for index in 0..logits.len() {
            let mut up = logits.clone();
            up[index] += h;
            let mut down = logits.clone();
            down[index] -= h;
            let numeric = (loss(&up) - loss(&down)) / (2.0 * h);
            assert!(
                (grad[index] - numeric).abs() < 1e-3,
                "softmax_backward[{index}]：解析 {} vs 数值 {numeric}",
                grad[index]
            );
        }
    }

    #[test]
    #[should_panic(expected = "越界")]
    fn cross_entropy_rejects_out_of_range_label() {
        let _ = cross_entropy(&[0.1, 0.2], 5);
    }
}
