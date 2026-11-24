//! 带梯度 SGD 的单遍：量化 `q`、累加 `dX`/`qdot`、更新 latent 与 scale 共用一次行扫描。
//!
//! 旧路径是 `fused::backward`（读 latent 量化）→ `dense::sgd_update`（再读 latent 做
//! STE 更新）；这里把两遍合成一遍。`q`/`dX`/`qdot` 的量化与累加顺序和
//! [`super::backward`] 相同，STE 门控与 `crate::dense` 的 8-lane 更新相同。

#[cfg(target_arch = "x86_64")]
use super::super::backward::has_avx2;
use crate::quant::Ste;

/// 返回 `dX`，并在同一遍里更新 `latent` 与 `scale`。
///
/// `dy[r] = ∂L/∂y[r]`；`scale[r]` 是更新前的值。`dX[c] = Σ_r dy[r]·scale[r]·q[r,c]`。
#[allow(clippy::too_many_arguments)]
pub fn backward_step(
    latent: &mut [f32],
    scale: &mut [f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    dy: &[f32],
    lr: f32,
    ste: Ste,
) -> Vec<f32> {
    assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
    assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    assert_eq!(dy.len(), rows, "dy 长度必须是 rows");
    #[cfg(target_arch = "x86_64")]
    if cols >= 8 && has_avx2() {
        // SAFETY: 长度已断言；内核只按 8 的整块和尾部访问。
        return unsafe { simd::backward_step(latent, scale, rows, cols, x, dy, lr, ste) };
    }
    backward_step_scalar(latent, scale, rows, cols, x, dy, lr, ste)
}

#[allow(clippy::too_many_arguments)]
fn backward_step_scalar(
    latent: &mut [f32],
    scale: &mut [f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    dy: &[f32],
    lr: f32,
    ste: Ste,
) -> Vec<f32> {
    let mut acc: [Vec<f32>; 4] = std::array::from_fn(|_| vec![0.0f32; cols]);
    let mut qdot = vec![0.0f32; rows];
    for r in 0..rows {
        let s_orig = scale[r];
        let s = s_orig.max(f32::MIN_POSITIVE);
        let coeff = dy[r] * s_orig;
        let a = &mut acc[r & 3];
        for c in 0..cols {
            let idx = r * cols + c;
            let w = latent[idx];
            let q = (w / s).round().clamp(-1.0, 1.0);
            qdot[r] += q * x[c];
            if q > 0.0 {
                a[c] += coeff;
            } else if q < 0.0 {
                a[c] -= coeff;
            }
            latent[idx] = w - lr * ste.backward(w, s_orig, coeff * x[c]);
        }
        scale[r] = (s_orig - lr * dy[r] * qdot[r]).max(1e-4);
    }
    let mut dx = vec![0.0f32; cols];
    for (c, slot) in dx.iter_mut().enumerate() {
        *slot = acc[0][c] + acc[1][c] + acc[2][c] + acc[3][c];
    }
    dx
}

#[cfg(target_arch = "x86_64")]
mod simd {
    #![allow(unsafe_op_in_unsafe_fn)]
    use std::arch::x86_64::*;

    use crate::quant::Ste;

    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn hsum(v: __m256) -> f32 {
        let lo = _mm256_castps256_ps128(v);
        let hi = _mm256_extractf128_ps(v, 1);
        let s = _mm_add_ps(lo, hi);
        let s = _mm_hadd_ps(s, s);
        let s = _mm_hadd_ps(s, s);
        _mm_cvtss_f32(s)
    }

    /// # Safety
    /// 需要 AVX2；四个切片长度已由调用方断言，内核只按 8 的整块和尾部访问。
    #[allow(clippy::too_many_arguments)]
    #[target_feature(enable = "avx2")]
    pub unsafe fn backward_step(
        latent: &mut [f32],
        scale: &mut [f32],
        rows: usize,
        cols: usize,
        x: &[f32],
        dy: &[f32],
        lr: f32,
        ste: Ste,
    ) -> Vec<f32> {
        let full = cols / 8;
        let mut acc: [Vec<f32>; 4] = std::array::from_fn(|_| vec![0.0f32; cols]);
        let mut qdot = vec![0.0f32; rows];
        let izero = _mm256_setzero_si256();
        let one = _mm256_set1_ps(1.0);
        let neg_one = _mm256_set1_ps(-1.0);
        let half = _mm256_set1_ps(0.5);
        let sign = _mm256_set1_ps(-0.0);
        let lrv = _mm256_set1_ps(lr);
        for r in 0..rows {
            let s_orig = scale[r];
            let s = s_orig.max(f32::MIN_POSITIVE);
            let sv = _mm256_set1_ps(s);
            let iv = _mm256_set1_ps(1.0 / s);
            let coeff = dy[r] * s_orig;
            let cv = _mm256_set1_ps(coeff);
            let row = &mut latent[r * cols..(r + 1) * cols];
            let a = &mut acc[r & 3];
            let mut dacc = [_mm256_setzero_ps(); 4];
            let mut block = |j: usize| -> __m256 {
                let base = j * 8;
                let wv = _mm256_loadu_ps(row.as_ptr().add(base));
                let scaled = _mm256_div_ps(wv, sv);
                let signed_half = _mm256_or_ps(_mm256_and_ps(scaled, sign), half);
                let rounded = _mm256_round_ps::<{ _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC }>(
                    _mm256_add_ps(scaled, signed_half),
                );
                let clamped = _mm256_min_ps(one, _mm256_max_ps(neg_one, rounded));
                let qi = _mm256_cvtps_epi32(clamped);
                let p = _mm256_cmpgt_epi32(qi, izero);
                let n = _mm256_cmpgt_epi32(izero, qi);
                let xv = _mm256_loadu_ps(x.as_ptr().add(base));
                let qx = _mm256_sub_ps(
                    _mm256_and_ps(xv, _mm256_castsi256_ps(p)),
                    _mm256_and_ps(xv, _mm256_castsi256_ps(n)),
                );
                let mut v = _mm256_loadu_ps(a.as_ptr().add(base));
                v = _mm256_add_ps(v, _mm256_and_ps(cv, _mm256_castsi256_ps(p)));
                v = _mm256_sub_ps(v, _mm256_and_ps(cv, _mm256_castsi256_ps(n)));
                _mm256_storeu_ps(a.as_mut_ptr().add(base), v);

                let ratio = _mm256_mul_ps(wv, iv);
                let base_grad = _mm256_mul_ps(cv, xv);
                let grad = match ste {
                    Ste::Identity => base_grad,
                    Ste::Clipped { ratio: limit } => {
                        let abs = _mm256_andnot_ps(sign, ratio);
                        let mask = _mm256_cmp_ps(abs, _mm256_set1_ps(limit), _CMP_LE_OQ);
                        _mm256_and_ps(base_grad, mask)
                    }
                    Ste::Soft => {
                        let sq = _mm256_mul_ps(ratio, ratio);
                        let g = _mm256_max_ps(_mm256_setzero_ps(), _mm256_sub_ps(one, sq));
                        let ordered = _mm256_cmp_ps(ratio, ratio, _CMP_ORD_Q);
                        _mm256_and_ps(_mm256_mul_ps(base_grad, g), ordered)
                    }
                };
                let next = _mm256_sub_ps(wv, _mm256_mul_ps(lrv, grad));
                _mm256_storeu_ps(row.as_mut_ptr().add(base), next);
                qx
            };
            let mut j = 0;
            while j + 4 <= full {
                dacc[0] = _mm256_add_ps(dacc[0], block(j));
                dacc[1] = _mm256_add_ps(dacc[1], block(j + 1));
                dacc[2] = _mm256_add_ps(dacc[2], block(j + 2));
                dacc[3] = _mm256_add_ps(dacc[3], block(j + 3));
                j += 4;
            }
            while j < full {
                dacc[0] = _mm256_add_ps(dacc[0], block(j));
                j += 1;
            }
            qdot[r] = hsum(_mm256_add_ps(
                _mm256_add_ps(dacc[0], dacc[1]),
                _mm256_add_ps(dacc[2], dacc[3]),
            ));
            for c in full * 8..cols {
                let w = row[c];
                let q = (w / s).round().clamp(-1.0, 1.0);
                qdot[r] += q * x[c];
                if q > 0.0 {
                    a[c] += coeff;
                } else if q < 0.0 {
                    a[c] -= coeff;
                }
                row[c] = w - lr * ste.backward(w, s_orig, coeff * x[c]);
            }
            scale[r] = (s_orig - lr * dy[r] * qdot[r]).max(1e-4);
        }
        let mut dx = vec![0.0f32; cols];
        for (c, slot) in dx.iter_mut().enumerate() {
            *slot = acc[0][c] + acc[1][c] + acc[2][c] + acc[3][c];
        }
        dx
    }
}

#[cfg(test)]
mod tests;
