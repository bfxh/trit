//! 带 STE 的三值线性层（QAT 风格）：`y = scale ⊙ (Q ⊙ x)`。
//!
//! - latent 权重是 f32；前向把 `q = clamp(round(w/scale), -1, 1)` 当权重；
//! - 反向用 [`Ste`] 把 `∂L/∂q` 直通回 latent，scale 也一起学；
//! - 训练是单样本 SGD——这是 M0 的收敛性/性能探针，不是训练框架。

use crate::quant::{Ste, quantize_with_scale, scale_grad};

/// Adam 优化器状态：与一组参数一一对应。
///
/// 只存一阶/二阶矩与步数；`lr`、`beta`、`eps` 是构造时固定的超参数。
#[derive(Clone, Debug)]
pub struct Adam {
    lr: f32,
    beta1: f32,
    beta2: f32,
    eps: f32,
    step: u32,
    bias1: f32,
    bias2: f32,
    m: Vec<f32>,
    v: Vec<f32>,
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

    fn begin(&mut self) {
        self.step = self.step.saturating_add(1);
        let t = self.step as i32;
        self.bias1 = 1.0 - self.beta1.powi(t);
        self.bias2 = 1.0 - self.beta2.powi(t);
    }

    fn apply(&mut self, index: usize, grad: f32, value: f32) -> f32 {
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

    /// 前向：`y[r] = scale[r] * Σ q[r,c]·x[c]`。
    pub fn forward(&self, x: &[f32]) -> Vec<f32> {
        assert_eq!(x.len(), self.cols, "输入长度必须是 cols");
        (0..self.rows)
            .map(|r| {
                let row = &self.latent[r * self.cols..(r + 1) * self.cols];
                let q = quantize_with_scale(row, self.scale[r]);
                self.scale[r] * dot(&q, x, self.prefetch)
            })
            .collect()
    }

    /// 单样本 SGD 一步，返回更新前的 MSE。
    pub fn train_step(&mut self, x: &[f32], target: &[f32]) -> f32 {
        assert_eq!(x.len(), self.cols, "输入长度必须是 cols");
        assert_eq!(target.len(), self.rows, "目标长度必须是 rows");

        let y = self.forward(x);
        let mut dy = vec![0.0f32; self.rows];
        let mut loss = 0.0f32;
        for (r, slot) in dy.iter_mut().enumerate() {
            let diff = y[r] - target[r];
            *slot = diff;
            loss += 0.5 * diff * diff;
        }
        loss /= self.rows as f32;

        let (lr, ste, cols) = (self.lr, self.ste, self.cols);
        for (r, (&dy_r, scale_slot)) in dy.iter().zip(self.scale.iter_mut()).enumerate() {
            let row = &self.latent[r * cols..(r + 1) * cols];
            let q = quantize_with_scale(row, *scale_slot);
            let scale = *scale_slot;
            // ∂L/∂scale = dy · Σ q·x；scale 必须保持为正。
            *scale_slot = (scale - lr * dy_r * scale_grad(&q, x)).max(1e-4);
            let base = r * cols;
            for (c, &xc) in x.iter().enumerate() {
                let grad_q = dy_r * scale * xc;
                let idx = base + c;
                let grad = ste.backward(self.latent[idx], scale, grad_q);
                self.latent[idx] -= lr * grad;
            }
        }
        loss
    }

    /// 用输出梯度更新一层，并返回输入梯度（供上一层反向）。
    ///
    /// `output_grad[r] = ∂L/∂y[r]`；内部按 STE 更新 latent/scale，返回
    /// `∂L/∂x[c] = Σ_r output_grad[r] · scale[r] · q[r,c]`（用更新前的三值权重）。
    pub fn train_step_with_grad(&mut self, input: &[f32], output_grad: &[f32]) -> Vec<f32> {
        assert_eq!(input.len(), self.cols, "输入长度必须是 cols");
        assert_eq!(output_grad.len(), self.rows, "梯度长度必须是 rows");
        let mut input_grad = vec![0.0f32; self.cols];
        let (lr, ste, cols) = (self.lr, self.ste, self.cols);
        for (r, &dy_r) in output_grad.iter().enumerate() {
            let scale = self.scale[r];
            let row = &self.latent[r * cols..(r + 1) * cols];
            let q = quantize_with_scale(row, scale);
            for (c, &xc) in input.iter().enumerate() {
                input_grad[c] += dy_r * scale * f32::from(q[c]);
                let grad_q = dy_r * scale * xc;
                let idx = r * cols + c;
                let grad = ste.backward(self.latent[idx], scale, grad_q);
                self.latent[idx] -= lr * grad;
            }
            self.scale[r] = (scale - lr * dy_r * scale_grad(&q, input)).max(1e-4);
        }
        input_grad
    }

    /// 用 Adam 更新一层，并返回输入梯度（供上一层反向）。
    ///
    /// 三值量化与 STE 口径与 [`Self::train_step_with_grad`] 相同；`latent_opt` /
    /// `scale_opt` 的参数个数必须分别是 `rows * cols` / `rows`。
    pub fn train_step_with_grad_adam(
        &mut self,
        input: &[f32],
        output_grad: &[f32],
        latent_opt: &mut Adam,
        scale_opt: &mut Adam,
    ) -> Vec<f32> {
        assert_eq!(input.len(), self.cols, "输入长度必须是 cols");
        assert_eq!(output_grad.len(), self.rows, "梯度长度必须是 rows");
        assert_eq!(latent_opt.len(), self.latent.len(), "latent 优化器大小");
        assert_eq!(scale_opt.len(), self.rows, "scale 优化器大小");
        latent_opt.begin();
        scale_opt.begin();
        let (ste, cols) = (self.ste, self.cols);
        let mut input_grad = vec![0.0f32; self.cols];
        for (r, &dy_r) in output_grad.iter().enumerate() {
            let scale = self.scale[r];
            let row = &self.latent[r * cols..(r + 1) * cols];
            let q = quantize_with_scale(row, scale);
            for (c, &xc) in input.iter().enumerate() {
                input_grad[c] += dy_r * scale * f32::from(q[c]);
                let grad_q = dy_r * scale * xc;
                let idx = r * cols + c;
                let grad = ste.backward(self.latent[idx], scale, grad_q);
                self.latent[idx] = latent_opt.apply(idx, grad, self.latent[idx]);
            }
            let grad_scale = dy_r * scale_grad(&q, input);
            self.scale[r] = scale_opt.apply(r, grad_scale, scale).max(1e-4);
        }
        input_grad
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
    fn adam_first_step_matches_closed_form() {
        let mut layer = TernaryLinear::new(1, 1, |_, _| 1.0, Ste::Identity, 0.0, 1.0);
        let mut latent_opt = Adam::new(1, 0.1);
        let mut scale_opt = Adam::new(1, 0.0);
        // 输入 1、输出梯度 2 ⇒ ∂L/∂latent = 2；第一步 mhat=2、vhat=4。
        layer.train_step_with_grad_adam(&[1.0], &[2.0], &mut latent_opt, &mut scale_opt);
        let got = layer.latent()[0];
        assert!(
            (got - 0.9).abs() < 1e-5,
            "latent={got}（偏置校正后应为 1 - 0.1 = 0.9）"
        );
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
