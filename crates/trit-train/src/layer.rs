//! 带 STE 的三值线性层（QAT 风格）：`y = scale ⊙ (Q ⊙ x)`。
//!
//! - latent 权重是 f32；前向把 `q = clamp(round(w/scale), -1, 1)` 当权重；
//! - 反向用 [`Ste`] 把 `∂L/∂q` 直通回 latent，scale 也一起学；
//! - 训练是单样本 SGD——这是 M0 的收敛性/性能探针，不是训练框架。

use crate::plane::PlaneMatrix;
use crate::quant::{Ste, quantize_with_scale};

mod adam;
mod grad;
#[cfg(test)]
mod packed_tests;
mod scalar;

/// Adam 优化器状态：与一组参数一一对应。
///
/// 只存一阶/二阶矩与步数；`lr`、`beta`、`eps` 是构造时固定的超参数。
#[derive(Clone, Debug)]
pub struct Adam {
    pub(crate) lr: f32,
    pub(crate) beta1: f32,
    pub(crate) beta2: f32,
    pub(crate) eps: f32,
    step: u32,
    pub(crate) bias1: f32,
    pub(crate) bias2: f32,
    pub(crate) m: Vec<f32>,
    pub(crate) v: Vec<f32>,
}

impl Adam {
    /// 新建：`size` 是参数个数，`lr` 是步长。
    pub fn new(size: usize, lr: f32) -> Self {
        Self {
            lr,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            step: 0,
            bias1: 1.0,
            bias2: 1.0,
            m: vec![0.0; size],
            v: vec![0.0; size],
        }
    }

    /// 参数个数。
    pub fn len(&self) -> usize {
        self.m.len()
    }

    /// 是否没有参数。
    pub fn is_empty(&self) -> bool {
        self.m.is_empty()
    }

    /// 已执行步数。
    pub fn step(&self) -> u32 {
        self.step
    }

    pub(crate) fn begin(&mut self) {
        self.step = self.step.saturating_add(1);
        let t = self.step as i32;
        self.bias1 = 1.0 - self.beta1.powi(t);
        self.bias2 = 1.0 - self.beta2.powi(t);
    }

    pub(crate) fn apply(&mut self, index: usize, grad: f32, value: f32) -> f32 {
        let m = &mut self.m[index];
        *m = self.beta1 * *m + (1.0 - self.beta1) * grad;
        let v = &mut self.v[index];
        *v = self.beta2 * *v + (1.0 - self.beta2) * grad * grad;
        let m_hat = *m / self.bias1;
        let v_hat = *v / self.bias2;
        value - self.lr * m_hat / (v_hat.sqrt() + self.eps)
    }
}

/// 一个三值线性层。
#[derive(Clone, Debug)]
pub struct TernaryLinear {
    rows: usize,
    cols: usize,
    latent: Vec<f32>,
    scale: Vec<f32>,
    ste: Ste,
    lr: f32,
    prefetch: bool,
}

impl TernaryLinear {
    /// 新建：`init(r, c)` 给 latent 初值，`scale0` 是每行初始 scale。
    pub fn new(
        rows: usize,
        cols: usize,
        mut init: impl FnMut(usize, usize) -> f32,
        ste: Ste,
        lr: f32,
        scale0: f32,
    ) -> Self {
        let mut latent = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                latent.push(init(r, c));
            }
        }
        Self {
            rows,
            cols,
            latent,
            scale: vec![scale0; rows],
            ste,
            lr,
            prefetch: true,
        }
    }

    /// 行数。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 列数。
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// latent 权重（行主序）。
    pub fn latent(&self) -> &[f32] {
        &self.latent
    }

    /// 每行 scale。
    pub fn scales(&self) -> &[f32] {
        &self.scale
    }

    /// 是否在热点循环里发软件预取。
    pub fn set_prefetch(&mut self, on: bool) {
        self.prefetch = on;
    }

    /// 第 `r` 行的三值权重。
    pub fn quantized_row(&self, r: usize) -> Vec<i8> {
        quantize_with_scale(
            &self.latent[r * self.cols..(r + 1) * self.cols],
            self.scale[r],
        )
    }

    /// 全部三值权重。
    pub fn quantized(&self) -> Vec<Vec<i8>> {
        (0..self.rows).map(|r| self.quantized_row(r)).collect()
    }

    /// 把当前 latent 量化一次，打成双导轨位平面。
    ///
    /// 训练/前向热路径已改用 `plane::fused` 单遍；这里保留为公开的位平面
    /// 检查与对拍入口，`from_latent` 一步完成量化与打包，不再逐行分配 `q`。
    pub fn pack(&self) -> PlaneMatrix {
        PlaneMatrix::from_latent(&self.latent, &self.scale, self.rows, self.cols)
    }

    /// 前向：`y[r] = scale[r] * Σ q[r,c]·x[c]`，量化与点积走 `fused::forward_dot`。
    pub fn forward(&self, x: &[f32]) -> Vec<f32> {
        assert_eq!(x.len(), self.cols, "输入长度必须是 cols");
        let qdot =
            crate::plane::fused::forward_dot(&self.latent, &self.scale, self.rows, self.cols, x);
        qdot.iter().zip(&self.scale).map(|(&d, &s)| s * d).collect()
    }

    /// 单样本 SGD 一步，返回更新前的 MSE。
    ///
    /// 前向用 `fused::forward_dot` 一次量化+点积，scale 梯度复用 `qdot`；`dW` 走 `dense::sgd_update` 融合更新。
    pub fn train_step(&mut self, x: &[f32], target: &[f32]) -> f32 {
        assert_eq!(x.len(), self.cols, "输入长度必须是 cols");
        assert_eq!(target.len(), self.rows, "目标长度必须是 rows");
        let qdot =
            crate::plane::fused::forward_dot(&self.latent, &self.scale, self.rows, self.cols, x);
        let y: Vec<f32> = qdot.iter().zip(&self.scale).map(|(&d, &s)| s * d).collect();
        let (loss, dy) = self.loss_and_grad(&y, target);

        let (lr, ste, cols) = (self.lr, self.ste, self.cols);
        crate::dense::sgd_update(
            &mut self.latent,
            x,
            self.rows,
            cols,
            &dy,
            &self.scale,
            lr,
            ste,
        );
        for (r, &dy_r) in dy.iter().enumerate() {
            self.scale[r] = (self.scale[r] - lr * dy_r * qdot[r]).max(1e-4);
        }
        loss
    }
}

/// `q·x`，`q ∈ {-1,0,+1}`；可选软件预取（每 16 个元素预取 32 个后）。
#[inline]
pub fn dot(q: &[i8], x: &[f32], prefetch: bool) -> f32 {
    let mut acc = 0.0f32;
    for (i, (&qi, &xi)) in q.iter().zip(x).enumerate() {
        if prefetch && i.is_multiple_of(16) {
            let next = i + 32;
            if next < q.len() {
                crate::prefetch::prefetch(q[next..].as_ptr());
                crate::prefetch::prefetch(x[next..].as_ptr());
            }
        }
        acc += f32::from(qi) * xi;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目标权重（三值）+ 由它生成的线性数据。
    fn target(r: usize, c: usize) -> f32 {
        match (r, c) {
            (0, 0) => 1.0,
            (0, 2) => -1.0,
            (1, 1) => 1.0,
            (1, 2) => 1.0,
            _ => 0.0,
        }
    }

    fn sample_x(seed: &mut u64, cols: usize) -> Vec<f32> {
        (0..cols)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                ((*seed % 2001) as f32 - 1000.0) / 1000.0
            })
            .collect()
    }

    fn train_all(ste: Ste, steps: usize) -> (TernaryLinear, f32) {
        let (rows, cols) = (2usize, 3usize);
        let mut state = 0x1234_5678_9abc_def1u64;
        let mut layer = TernaryLinear::new(
            rows,
            cols,
            |_, _| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state % 100) as f32 - 50.0) * 0.001
            },
            ste,
            0.05,
            0.5,
        );
        let mut loss = f32::NAN;
        let mut data = 0xdead_beef_0bad_f00du64;
        for _ in 0..steps {
            let x = sample_x(&mut data, cols);
            let y: Vec<f32> = (0..rows)
                .map(|r| (0..cols).map(|c| target(r, c) * x[c]).sum())
                .collect();
            loss = layer.train_step(&x, &y);
        }
        (layer, loss)
    }

    #[test]
    fn identity_ste_learns_ternary_targets() {
        let (layer, loss) = train_all(Ste::Identity, 4000);
        assert!(loss < 0.02, "loss={loss}");
        let q = layer.quantized();
        for (r, row) in q.iter().enumerate() {
            for (c, &got) in row.iter().enumerate() {
                assert_eq!(f32::from(got), target(r, c), "q[{r}][{c}] = {got}");
            }
        }
        assert!(
            (layer.scales()[0] - 1.0).abs() < 0.2,
            "scale={:?}",
            layer.scales()
        );
    }

    #[test]
    fn clipped_and_soft_also_converge() {
        for ste in [Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
            let (layer, loss) = train_all(ste, 4000);
            assert!(loss < 0.05, "{ste:?}: loss={loss}");
            let q = layer.quantized();
            assert_eq!(q[0][0], 1, "{ste:?}");
            assert_eq!(q[0][2], -1, "{ste:?}");
        }
    }

    #[test]
    fn forward_is_exact_for_quantized_weights() {
        let (layer, _) = train_all(Ste::Identity, 4000);
        let mut state = 42u64;
        let x = sample_x(&mut state, 3);
        let y = layer.forward(&x);
        let want: Vec<f32> = (0..2)
            .map(|r| layer.scales()[r] * (0..3).map(|c| target(r, c) * x[c]).sum::<f32>())
            .collect();
        for (got, want) in y.iter().zip(&want) {
            assert!((got - want).abs() < 1e-5, "{got} vs {want}");
        }
    }

    #[test]
    fn grad_step_matches_train_step_for_output_layer() {
        let mut a = TernaryLinear::new(
            2,
            3,
            |r, c| ((r * 7 + c * 5) as f32) / 9.0 - 0.4,
            Ste::Identity,
            0.05,
            1.0,
        );
        let mut b = a.clone();
        let input = [0.5, -1.0, 0.25];
        let target = [0.2, -0.1];
        let y = b.forward(&input);
        let dy: Vec<f32> = y.iter().zip(&target).map(|(y, t)| y - t).collect();
        a.train_step(&input, &target);
        let _ = b.train_step_with_grad(&input, &dy);
        for (x, y) in a.latent().iter().zip(b.latent()) {
            assert!((x - y).abs() < 1e-6);
        }
        for (x, y) in a.scales().iter().zip(b.scales()) {
            assert!((x - y).abs() < 1e-6);
        }
    }

    #[test]
    fn grad_step_returns_input_gradient() {
        let mut layer = TernaryLinear::new(
            2,
            2,
            |r, c| if r == c { 1.0 } else { 0.0 },
            Ste::Identity,
            0.0,
            1.0,
        );
        let input = [2.0, -3.0];
        let grad = layer.train_step_with_grad(&input, &[1.0, -1.0]);
        assert_eq!(grad, vec![1.0, -1.0]);
    }

    #[test]
    fn plane_reverse_matches_weights_before_update() {
        let mut layer = TernaryLinear::new(
            7,
            13,
            |r, c| ((r * 11 + c * 7) as f32) / 23.0 - 0.7,
            Ste::Identity,
            0.0,
            0.5,
        );
        let q = layer.quantized();
        let scale = layer.scales().to_vec();
        let input: Vec<f32> = (0..13).map(|i| (i as f32 - 6.0) / 7.0).collect();
        let dy: Vec<f32> = (0..7).map(|r| (r as f32 - 3.0) / 3.0).collect();
        let want: Vec<f32> = (0..13)
            .map(|c| (0..7).map(|r| dy[r] * scale[r] * f32::from(q[r][c])).sum())
            .collect();
        let got = layer.train_step_with_grad(&input, &dy);
        for (g, w) in got.iter().zip(&want) {
            assert!((g - w).abs() <= 1e-4 * (1.0 + w.abs()), "dX={g} want={w}");
        }
    }

    #[test]
    fn plane_reverse_matches_with_adam() {
        let mut layer = TernaryLinear::new(
            5,
            17,
            |r, c| ((r * 13 + c * 3) as f32) / 29.0 - 0.5,
            Ste::Identity,
            0.05,
            0.5,
        );
        let q = layer.quantized();
        let scale = layer.scales().to_vec();
        let input: Vec<f32> = (0..17).map(|i| (i as f32 - 8.0) / 9.0).collect();
        let dy: Vec<f32> = (0..5).map(|r| (r as f32 - 2.0) / 2.0).collect();
        let want: Vec<f32> = (0..17)
            .map(|c| (0..5).map(|r| dy[r] * scale[r] * f32::from(q[r][c])).sum())
            .collect();
        let mut latent_opt = Adam::new(5 * 17, 0.01);
        let mut scale_opt = Adam::new(5, 0.01);
        let got = layer.train_step_with_grad_adam(&input, &dy, &mut latent_opt, &mut scale_opt);
        for (g, w) in got.iter().zip(&want) {
            assert!((g - w).abs() <= 1e-4 * (1.0 + w.abs()), "dX={g} want={w}");
        }
    }

    #[test]
    fn pack_matches_quantized_rows() {
        let layer = TernaryLinear::new(
            4,
            9,
            |r, c| ((r * 5 + c * 7) as f32) / 17.0 - 0.4,
            Ste::Identity,
            0.05,
            0.5,
        );
        let packed = layer.pack();
        let flat: Vec<i8> = layer.quantized().into_iter().flatten().collect();
        assert_eq!(packed.q(), flat.as_slice());
    }

    fn train_all_adam(steps: usize) -> (TernaryLinear, f32) {
        let (rows, cols) = (2usize, 3usize);
        let mut layer = TernaryLinear::new(
            rows,
            cols,
            |r, c| ((r * 7 + c * 5) as f32) / 9.0 - 0.4,
            Ste::Identity,
            0.05,
            0.5,
        );
        let mut latent_opt = Adam::new(rows * cols, 0.05);
        let mut scale_opt = Adam::new(rows, 0.005);
        let mut data = 0xdead_beef_0bad_f00du64;
        for _ in 0..steps {
            let x = sample_x(&mut data, cols);
            let y: Vec<f32> = (0..rows)
                .map(|r| (0..cols).map(|c| target(r, c) * x[c]).sum())
                .collect();
            let out = layer.forward(&x);
            let dy: Vec<f32> = out.iter().zip(&y).map(|(o, t)| o - t).collect();
            layer.train_step_with_grad_adam(&x, &dy, &mut latent_opt, &mut scale_opt);
        }
        let x = sample_x(&mut data, cols);
        let y: Vec<f32> = (0..rows)
            .map(|r| (0..cols).map(|c| target(r, c) * x[c]).sum())
            .collect();
        let out = layer.forward(&x);
        let loss = out
            .iter()
            .zip(&y)
            .map(|(o, t)| 0.5 * (o - t).powi(2))
            .sum::<f32>()
            / rows as f32;
        (layer, loss)
    }

    #[test]
    fn adam_learns_ternary_targets() {
        let (layer, loss) = train_all_adam(4000);
        assert!(loss < 0.05, "loss={loss}");
        let q = layer.quantized();
        let hits: usize = q
            .iter()
            .enumerate()
            .map(|(r, row)| {
                row.iter()
                    .enumerate()
                    .filter(|&(c, &got)| f32::from(got) == target(r, c))
                    .count()
            })
            .sum();
        assert!(hits >= 4, "q={q:?}");
    }

    #[test]
    fn adam_counts_steps_per_layer_call() {
        let mut layer = TernaryLinear::new(2, 3, |_, _| 0.1, Ste::Identity, 0.05, 0.5);
        let mut latent_opt = Adam::new(6, 0.05);
        let mut scale_opt = Adam::new(2, 0.005);
        assert_eq!(latent_opt.step(), 0);
        for _ in 0..3 {
            layer.train_step_with_grad_adam(
                &[1.0, 0.0, 0.0],
                &[1.0, -1.0],
                &mut latent_opt,
                &mut scale_opt,
            );
        }
        assert_eq!(latent_opt.step(), 3);
        assert_eq!(scale_opt.step(), 3);
    }

    #[test]
    #[should_panic(expected = "latent 优化器大小")]
    fn adam_rejects_wrong_latent_state() {
        let mut layer = TernaryLinear::new(2, 3, |_, _| 0.1, Ste::Identity, 0.05, 0.5);
        let mut latent_opt = Adam::new(5, 0.05);
        let mut scale_opt = Adam::new(2, 0.005);
        layer.train_step_with_grad_adam(&[0.0; 3], &[0.0; 2], &mut latent_opt, &mut scale_opt);
    }
}
