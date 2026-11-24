//! 量化点积融合：SGD `train_step` 只消费 `q·x`，不需要双导轨。
//!
//! 每 8 列直接做 `clamp(round(w / scale), -1, 1)`，用符号掩码乘 `x` 并累加；
//! 与 `pack()` + `row_dot` 的量化、累加分组和 `hsum` 顺序一致，输出逐位相同。
#[cfg(target_arch = "x86_64")]
use super::backward::has_avx2;

/// 逐行返回 `Σ_c q[r,c] * x[c]`，其中 `q = clamp(round(w / scale), -1, 1)`。
#[inline]
pub fn forward_dot(latent: &[f32], scale: &[f32], rows: usize, cols: usize, x: &[f32]) -> Vec<f32> {
    assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
    assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    #[cfg(target_arch = "x86_64")]
    if cols >= 8 && has_avx2() {
        // SAFETY: 长度已断言；内核按 8 列整块与尾部访问。
        return unsafe { simd::forward_dot(latent, scale, rows, cols, x) };
    }
    forward_dot_scalar(latent, scale, rows, cols, x)
}

fn forward_dot_scalar(
    latent: &[f32],
    scale: &[f32],
    rows: usize,
    cols: usize,
    x: &[f32],
) -> Vec<f32> {
    let mut out = vec![0.0f32; rows];
    for (r, slot) in out.iter_mut().enumerate() {
        let s = scale[r].max(f32::MIN_POSITIVE);
        let row = &latent[r * cols..(r + 1) * cols];
        let mut sum = 0.0f32;
        for (&w, &xc) in row.iter().zip(x) {
            let q = (w / s).round().clamp(-1.0, 1.0);
            if q > 0.0 {
                sum += xc;
            } else if q < 0.0 {
                sum -= xc;
            }
        }
        *slot = sum;
    }
    out
}

#[cfg(target_arch = "x86_64")]
mod simd {
    #![allow(unsafe_op_in_unsafe_fn)]
    use std::arch::x86_64::*;

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

    #[target_feature(enable = "avx2")]
    unsafe fn row(latent: &[f32], s: f32, x: &[f32], cols: usize) -> f32 {
        let full = cols / 8;
        let sv = _mm256_set1_ps(s.max(f32::MIN_POSITIVE));
        let sign = _mm256_set1_ps(-0.0);
        let half = _mm256_set1_ps(0.5);
        let one = _mm256_set1_ps(1.0);
        let neg_one = _mm256_set1_ps(-1.0);
        let zero = _mm256_setzero_si256();
        let mut acc = [_mm256_setzero_ps(); 4];
        let dot = |j: usize| -> __m256 {
            let wv = _mm256_loadu_ps(latent.as_ptr().add(j * 8));
            let scaled = _mm256_div_ps(wv, sv);
            let signed_half = _mm256_or_ps(_mm256_and_ps(scaled, sign), half);
            let rounded = _mm256_round_ps::<{ _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC }>(
                _mm256_add_ps(scaled, signed_half),
            );
            let clamped = _mm256_min_ps(one, _mm256_max_ps(neg_one, rounded));
            let qi = _mm256_cvtps_epi32(clamped);
            let xv = _mm256_loadu_ps(x.as_ptr().add(j * 8));
            let p = _mm256_castsi256_ps(_mm256_cmpgt_epi32(qi, zero));
            let n = _mm256_castsi256_ps(_mm256_cmpgt_epi32(zero, qi));
            _mm256_sub_ps(_mm256_and_ps(xv, p), _mm256_and_ps(xv, n))
        };
        let mut j = 0;
        while j + 4 <= full {
            acc[0] = _mm256_add_ps(acc[0], dot(j));
            acc[1] = _mm256_add_ps(acc[1], dot(j + 1));
            acc[2] = _mm256_add_ps(acc[2], dot(j + 2));
            acc[3] = _mm256_add_ps(acc[3], dot(j + 3));
            j += 4;
        }
        while j < full {
            acc[0] = _mm256_add_ps(acc[0], dot(j));
            j += 1;
        }
        let mut sum = hsum(_mm256_add_ps(
            _mm256_add_ps(acc[0], acc[1]),
            _mm256_add_ps(acc[2], acc[3]),
        ));
        for c in full * 8..cols {
            let q = (latent[c] / s.max(f32::MIN_POSITIVE))
                .round()
                .clamp(-1.0, 1.0);
            if q > 0.0 {
                sum += x[c];
            } else if q < 0.0 {
                sum -= x[c];
            }
        }
        sum
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn forward_dot(
        latent: &[f32],
        scale: &[f32],
        rows: usize,
        cols: usize,
        x: &[f32],
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; rows];
        for (r, slot) in out.iter_mut().enumerate() {
            *slot = row(&latent[r * cols..(r + 1) * cols], scale[r], x, cols);
        }
        out
    }
}

#[cfg(test)]
mod tests;
