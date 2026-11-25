//! `forward_step` 的标量与 AVX2 整步内核；AVX2 路径把逐行 `has_avx2` 调度提到循环外。

use crate::dense;
use crate::quant::Ste;

#[allow(clippy::too_many_arguments)]
pub(super) fn train_step_scalar(
    latent: &mut [f32],
    scale: &mut [f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    target: &[f32],
    lr: f32,
    ste: Ste,
) -> f32 {
    let mut loss = 0.0f32;
    for r in 0..rows {
        let s = scale[r];
        let row = &mut latent[r * cols..(r + 1) * cols];
        let qdot = super::super::forward_dot_row(row, s, cols, x);
        let diff = s * qdot - target[r];
        loss += 0.5 * diff * diff;
        dense::sgd_update_row(row, x, diff * s, lr, s, ste);
        scale[r] = (s - lr * diff * qdot).max(1e-4);
    }
    loss / rows as f32
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn train_step_avx2(
    latent: &mut [f32],
    scale: &mut [f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    target: &[f32],
    lr: f32,
    ste: Ste,
) -> f32 {
    let mut loss = 0.0f32;
    for r in 0..rows {
        let s = scale[r];
        let row = &mut latent[r * cols..(r + 1) * cols];
        // SAFETY: 长度已断言；内核只按 8 列整块与尾部访问。
        let qdot = unsafe { super::super::simd::row(row, s, x, cols) };
        let diff = s * qdot - target[r];
        loss += 0.5 * diff * diff;
        // SAFETY: 同上，`row.len() == x.len() == cols >= 8`。
        unsafe { dense::sgd_update_row_avx2(row, x, diff * s, lr, s, ste) };
        scale[r] = (s - lr * diff * qdot).max(1e-4);
    }
    loss / rows as f32
}
