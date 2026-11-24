//! `TernaryLinear` 的 Adam 训练步：复用一份 planes 的 `dX`/scale，`dW` 走融合更新。
//!
//! 放在独立模块里，避免 `layer.rs` 热路径被挤动；SIMD 只在 x86_64 + AVX2 + 列数 ≥ 8 时启用，尾部与短行走标量。

use super::{Adam, TernaryLinear};
use crate::quant::Ste;

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
        let planes = self.pack();
        let mut input_grad = vec![0.0f32; self.cols];
        planes.input_grad(output_grad, &self.scale, &mut input_grad);
        adam_update(
            &mut self.latent,
            input,
            self.rows,
            self.cols,
            output_grad,
            &self.scale,
            self.ste,
            latent_opt,
        );
        for (r, &dy_r) in output_grad.iter().enumerate() {
            let grad_scale = dy_r * planes.row_dot(r, input);
            self.scale[r] = scale_opt.apply(r, grad_scale, self.scale[r]).max(1e-4);
        }
        input_grad
    }
}

/// 对整层 latent 做一次 Adam 外积更新（调用方需先 [`Adam::begin`]）。
///
/// 计时/调用口径：`dy`、`scale` 长度是 `rows`，`x` 长度是 `cols`，
/// `latent` 是行主序 `rows * cols`；`scale[r]` 是更新前的值。
#[allow(clippy::too_many_arguments)]
fn adam_update(
    latent: &mut [f32],
    x: &[f32],
    rows: usize,
    cols: usize,
    dy: &[f32],
    scale: &[f32],
    ste: Ste,
    opt: &mut Adam,
) {
    assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    assert_eq!(dy.len(), rows, "dy 长度必须是 rows");
    assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
    assert_eq!(opt.len(), rows * cols, "Adam 大小必须是 rows * cols");
    #[cfg(target_arch = "x86_64")]
    let params = simd::Params::of(opt);
    for r in 0..rows {
        let coeff = dy[r] * scale[r];
        let base = r * cols;
        let row = &mut latent[base..base + cols];
        #[cfg(target_arch = "x86_64")]
        if cols >= 8 && has_avx2() {
            let m = &mut opt.m[base..base + cols];
            let v = &mut opt.v[base..base + cols];
            // SAFETY: 四个切片长度都是 cols；内核只按 8 的整块和尾部访问。
            unsafe {
                simd::update_row(row, x, m, v, coeff, scale[r], params, ste);
            }
            continue;
        }
        for c in 0..cols {
            let w = row[c];
            let grad = ste.backward(w, scale[r], coeff * x[c]);
            let idx = base + c;
            row[c] = opt.apply(idx, grad, w);
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn has_avx2() -> bool {
    std::arch::is_x86_feature_detected!("avx2")
}

#[cfg(not(target_arch = "x86_64"))]
#[allow(dead_code)]
fn has_avx2() -> bool {
    false
}

#[cfg(target_arch = "x86_64")]
mod simd {
    #![allow(unsafe_op_in_unsafe_fn)]
    use std::arch::x86_64::*;

    use super::Adam;
    use crate::quant::Ste;

    #[derive(Clone, Copy)]
    pub(super) struct Params {
        lr: f32,
        beta1: f32,
        beta2: f32,
        eps: f32,
        bias1: f32,
        bias2: f32,
    }

    impl Params {
        pub(super) fn of(opt: &Adam) -> Self {
            Self {
                lr: opt.lr,
                beta1: opt.beta1,
                beta2: opt.beta2,
                eps: opt.eps,
                bias1: opt.bias1,
                bias2: opt.bias2,
            }
        }
    }

    /// 融合 `dW` 的 Adam 更新：16 个向量寄存器一次做 gate、矩更新和参数更新。
    ///
    /// # Safety
    /// 需要 AVX2；`row`、`x`、`m`、`v` 长度相同且至少 8 个元素。
    #[allow(clippy::too_many_arguments)]
    #[target_feature(enable = "avx2")]
    pub unsafe fn update_row(
        row: &mut [f32],
        x: &[f32],
        m: &mut [f32],
        v: &mut [f32],
        coeff: f32,
        scale: f32,
        params: Params,
        ste: Ste,
    ) {
        debug_assert_eq!(row.len(), x.len());
        debug_assert_eq!(row.len(), m.len());
        debug_assert_eq!(row.len(), v.len());
        let full = row.len() / 8;
        let inv_scale = 1.0 / scale.max(f32::MIN_POSITIVE);
        let cv = _mm256_set1_ps(coeff);
        let iv = _mm256_set1_ps(inv_scale);
        let one = _mm256_set1_ps(1.0);
        let zero = _mm256_setzero_ps();
        let sign = _mm256_set1_ps(-0.0);
        let b1 = _mm256_set1_ps(params.beta1);
        let b2 = _mm256_set1_ps(params.beta2);
        let one_b1 = _mm256_set1_ps(1.0 - params.beta1);
        let one_b2 = _mm256_set1_ps(1.0 - params.beta2);
        let bias1 = _mm256_set1_ps(params.bias1);
        let bias2 = _mm256_set1_ps(params.bias2);
        let lrv = _mm256_set1_ps(params.lr);
        let eps = _mm256_set1_ps(params.eps);
        for j in 0..full {
            let base = j * 8;
            // SAFETY: base + 8 <= full * 8 <= 各切片长度。
            let w = _mm256_loadu_ps(row.as_ptr().add(base));
            let xv = _mm256_loadu_ps(x.as_ptr().add(base));
            let ratio = _mm256_mul_ps(w, iv);
            let base_grad = _mm256_mul_ps(cv, xv);
            let grad = match ste {
                Ste::Identity => base_grad,
                Ste::Clipped { ratio: limit } => {
                    let abs = _mm256_andnot_ps(sign, ratio);
                    let mask = _mm256_cmp_ps(abs, _mm256_set1_ps(limit), _CMP_LE_OQ);
                    // 比较结果是全 1 / 全 0 的位掩码，不能当 1.0 浮点相乘。
                    _mm256_and_ps(base_grad, mask)
                }
                Ste::Soft => {
                    let sq = _mm256_mul_ps(ratio, ratio);
                    let g = _mm256_max_ps(zero, _mm256_sub_ps(one, sq));
                    // Rust 的 f32::max 把 NaN 折叠成 0；SIMD max 不保证，显式清掉 NaN。
                    let ordered = _mm256_cmp_ps(ratio, ratio, _CMP_ORD_Q);
                    _mm256_and_ps(_mm256_mul_ps(base_grad, g), ordered)
                }
            };
            let m_old = _mm256_loadu_ps(m.as_ptr().add(base));
            let v_old = _mm256_loadu_ps(v.as_ptr().add(base));
            let m_new = _mm256_add_ps(_mm256_mul_ps(b1, m_old), _mm256_mul_ps(one_b1, grad));
            let v_new = _mm256_add_ps(
                _mm256_mul_ps(b2, v_old),
                _mm256_mul_ps(one_b2, _mm256_mul_ps(grad, grad)),
            );
            let m_hat = _mm256_div_ps(m_new, bias1);
            let v_hat = _mm256_div_ps(v_new, bias2);
            let denom = _mm256_add_ps(_mm256_sqrt_ps(v_hat), eps);
            let next = _mm256_sub_ps(w, _mm256_div_ps(_mm256_mul_ps(lrv, m_hat), denom));
            // SAFETY: 同一块，base + 8 <= 各切片长度。
            _mm256_storeu_ps(row.as_mut_ptr().add(base), next);
            _mm256_storeu_ps(m.as_mut_ptr().add(base), m_new);
            _mm256_storeu_ps(v.as_mut_ptr().add(base), v_new);
        }
        for c in full * 8..row.len() {
            let grad = ste.backward(row[c], scale, coeff * x[c]);
            m[c] = params.beta1 * m[c] + (1.0 - params.beta1) * grad;
            v[c] = params.beta2 * v[c] + (1.0 - params.beta2) * grad * grad;
            let m_hat = m[c] / params.bias1;
            let v_hat = v[c] / params.bias2;
            row[c] -= params.lr * m_hat / (v_hat.sqrt() + params.eps);
        }
    }
}

#[cfg(test)]
mod tests;
