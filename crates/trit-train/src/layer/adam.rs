//! `TernaryLinear` 的 Adam 训练步：量化、`dX`/`qdot` 与行级 Adam `dW` 单遍。
//!
//! 放在独立模块里，避免 `layer.rs` 热路径被挤动；SIMD 只在 x86_64 + AVX2 + 列数 ≥ 8 时启用，尾部与短行走标量。
//! 旧两遍对拍用的 `adam_update` 在 `adam/support.rs`，只在测试构建里存在。

use super::{Adam, TernaryLinear};

impl TernaryLinear {
    /// 用 Adam 更新一层，并返回输入梯度（供上一层反向）。
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
        let params = crate::plane::fused::adam_step::AdamParams::new(
            latent_opt.lr,
            latent_opt.beta1,
            latent_opt.beta2,
            latent_opt.eps,
            latent_opt.bias1,
            latent_opt.bias2,
        );
        let (input_grad, qdot) = crate::plane::fused::adam_step::adam_step(
            &mut self.latent,
            &self.scale,
            self.rows,
            self.cols,
            input,
            output_grad,
            &mut latent_opt.m,
            &mut latent_opt.v,
            params,
            self.ste,
        );
        for (r, &dy_r) in output_grad.iter().enumerate() {
            let grad_scale = dy_r * qdot[r];
            self.scale[r] = scale_opt.apply(r, grad_scale, self.scale[r]).max(1e-4);
        }
        input_grad
    }
}

#[cfg(test)]
#[path = "adam/support.rs"]
mod support;

#[cfg(test)]
mod tests;
