//! 单行 `dW` 更新：整块 AVX2 与标量回退。
use super::has_avx2;
use crate::quant::Ste;

/// 单行 `dW` 更新；供 `train_step` 的逐行融合复用，逐位口径同 `dense::sgd_update`。
pub(crate) fn sgd_update_row(
    row: &mut [f32],
    x: &[f32],
    coeff: f32,
    lr: f32,
    scale: f32,
    ste: Ste,
) {
    #[cfg(target_arch = "x86_64")]
    if row.len() >= 8 && has_avx2() {
        // SAFETY: row.len() == x.len()，内核只按 8 的整块和尾部访问。
        unsafe {
            simd::update_row(row, x, coeff, lr, scale, ste);
        }
        return;
    }
    update_row_scalar(row, x, coeff, lr, scale, ste);
}

fn update_row_scalar(row: &mut [f32], x: &[f32], coeff: f32, lr: f32, scale: f32, ste: Ste) {
    for (w, &xc) in row.iter_mut().zip(x) {
        *w -= lr * ste.backward(*w, scale, coeff * xc);
    }
}

#[cfg(target_arch = "x86_64")]
mod simd {
    #![allow(unsafe_op_in_unsafe_fn)]
    use std::arch::x86_64::*;

    use crate::quant::Ste;

    /// 融合 `dW` 更新：`w - lr * coeff * x * gate`；尾部走 `Ste::backward`。
    ///
    /// # Safety
    /// 需要 AVX2；`row.len() == x.len()`，两者都至少 8 个元素。
    #[target_feature(enable = "avx2")]
    pub unsafe fn update_row(
        row: &mut [f32],
        x: &[f32],
        coeff: f32,
        lr: f32,
        scale: f32,
        ste: Ste,
    ) {
        debug_assert_eq!(row.len(), x.len());
        let full = row.len() / 8;
        let inv_scale = 1.0 / scale.max(f32::MIN_POSITIVE);
        let cv = _mm256_set1_ps(coeff);
        let lrv = _mm256_set1_ps(lr);
        let iv = _mm256_set1_ps(inv_scale);
        let one = _mm256_set1_ps(1.0);
        let zero = _mm256_setzero_ps();
        let sign = _mm256_set1_ps(-0.0);
        for j in 0..full {
            let base = j * 8;
            // SAFETY: base + 8 <= full * 8 <= row.len() == x.len()。
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
                    // 交换 max 的操作数：NaN 时 Intel max 返回第二个操作数 0。
                    let g = _mm256_max_ps(_mm256_sub_ps(one, sq), zero);
                    _mm256_mul_ps(base_grad, g)
                }
            };
            let next = _mm256_sub_ps(w, _mm256_mul_ps(lrv, grad));
            // SAFETY: 同一块，base + 8 <= row.len()。
            _mm256_storeu_ps(row.as_mut_ptr().add(base), next);
        }
        for c in full * 8..row.len() {
            row[c] -= lr * ste.backward(row[c], scale, coeff * x[c]);
        }
    }
}
