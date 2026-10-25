//! 带 STE 的三值线性层（QAT 风格）：`y = scale ⊙ (Q ⊙ x)`。
//!
//! - latent 权重是 f32；前向把 `q = clamp(round(w/scale), -1, 1)` 当权重；
//! - 反向用 [`Ste`] 把 `∂L/∂q` 直通回 latent，scale 也一起学；
//! - 训练是单样本 SGD——这是 M0 的收敛性/性能探针，不是训练框架。

use crate::quant::{Ste, quantize_with_scale, scale_grad};

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
}
