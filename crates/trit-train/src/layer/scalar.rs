//! `TernaryLinear` 的标量参考路径：逐行量化 + 标量点积 + 逐元素 STE。
//!
//! 只用于对拍与基准，不改变 `train_step` 的语义；放在单独模块里是为了让
//! `layer.rs` 的模块预算不再被参考实现撑大。

use super::{TernaryLinear, dot};
use crate::quant::{quantize_with_scale, scale_grad};

impl TernaryLinear {
    /// MSE 与输出梯度；`train_step` 的融合路径也复用它。
    pub(super) fn loss_and_grad(&self, y: &[f32], target: &[f32]) -> (f32, Vec<f32>) {
        debug_assert_eq!(y.len(), self.rows);
        debug_assert_eq!(target.len(), self.rows);
        let mut dy = vec![0.0f32; self.rows];
        let mut loss = 0.0f32;
        for (r, slot) in dy.iter_mut().enumerate() {
            let diff = y[r] - target[r];
            *slot = diff;
            loss += 0.5 * diff * diff;
        }
        (loss / self.rows as f32, dy)
    }

    /// 前向的标量参考：逐行量化后做标量点积。
    pub fn forward_scalar(&self, x: &[f32]) -> Vec<f32> {
        assert_eq!(x.len(), self.cols, "输入长度必须是 cols");
        (0..self.rows)
            .map(|r| {
                let row = &self.latent[r * self.cols..(r + 1) * self.cols];
                let q = quantize_with_scale(row, self.scale[r]);
                self.scale[r] * dot(&q, x, self.prefetch)
            })
            .collect()
    }

    /// 单样本 SGD 的标量参考路径：逐行量化 + 标量点积 + 逐元素 STE。
    ///
    /// 只用于对拍与基准，不改变 `train_step` 的语义。
    pub fn train_step_scalar(&mut self, x: &[f32], target: &[f32]) -> f32 {
        assert_eq!(x.len(), self.cols, "输入长度必须是 cols");
        assert_eq!(target.len(), self.rows, "目标长度必须是 rows");

        let y = self.forward_scalar(x);
        let (loss, dy) = self.loss_and_grad(&y, target);

        let (lr, ste, cols) = (self.lr, self.ste, self.cols);
        for (r, (&dy_r, scale_slot)) in dy.iter().zip(self.scale.iter_mut()).enumerate() {
            let row = &self.latent[r * cols..(r + 1) * cols];
            let q = quantize_with_scale(row, *scale_slot);
            let scale = *scale_slot;
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
