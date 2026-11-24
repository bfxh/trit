//! `TernaryLinear` 的带梯度 SGD 训练步：量化 + `dX`/`qdot` 单遍。
//!
//! 从 `layer.rs` 拆出，保持 `layer.rs` 的模块预算；量化内核在
//! [`crate::plane::fused::backward`]，标量参考在
//! [`crate::plane::PlaneMatrix::input_grad_scalar`]。

use super::{Adam, TernaryLinear};

impl TernaryLinear {
    /// 用输出梯度更新一层，并返回输入梯度（供上一层反向）。
    ///
    /// `output_grad[r] = ∂L/∂y[r]`；内部按 STE 更新 latent/scale，返回
    /// `∂L/∂x[c] = Σ_r output_grad[r] · scale[r] · q[r,c]`（用更新前的三值权重）。
    pub fn train_step_with_grad(&mut self, input: &[f32], output_grad: &[f32]) -> Vec<f32> {
        assert_eq!(input.len(), self.cols, "输入长度必须是 cols");
        assert_eq!(output_grad.len(), self.rows, "梯度长度必须是 rows");
        let (input_grad, qdot) = self.fused_backward(input, output_grad);
        let (lr, ste, cols) = (self.lr, self.ste, self.cols);
        crate::dense::sgd_update(
            &mut self.latent,
            input,
            self.rows,
            cols,
            output_grad,
            &self.scale,
            lr,
            ste,
        );
        for (r, &dy_r) in output_grad.iter().enumerate() {
            self.scale[r] = (self.scale[r] - lr * dy_r * qdot[r]).max(1e-4);
        }
        input_grad
    }

    /// Adam 的逐元素参考路径：用于对拍与基准；融合路径见
    /// [`TernaryLinear::train_step_with_grad_adam`]。
    pub fn train_step_with_grad_adam_scalar(
        &mut self,
        input: &[f32],
        output_grad: &[f32],
        latent_opt: &mut Adam,
        scale_opt: &mut Adam,
    ) -> Vec<f32> {
        latent_opt.begin();
        scale_opt.begin();
        let planes = self.pack();
        let (ste, cols) = (self.ste, self.cols);
        let mut input_grad = vec![0.0f32; self.cols];
        planes.input_grad(output_grad, &self.scale, &mut input_grad);
        for (r, &dy_r) in output_grad.iter().enumerate() {
            let scale = self.scale[r];
            let grad_scale = dy_r * planes.row_dot(r, input);
            self.scale[r] = scale_opt.apply(r, grad_scale, scale).max(1e-4);
            let base = r * cols;
            for (c, &xc) in input.iter().enumerate() {
                let grad_q = dy_r * scale * xc;
                let idx = base + c;
                let grad = ste.backward(self.latent[idx], scale, grad_q);
                self.latent[idx] = latent_opt.apply(idx, grad, self.latent[idx]);
            }
        }
        input_grad
    }
}
