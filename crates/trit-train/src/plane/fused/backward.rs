//! 量化 + `dX` + `qdot` 单遍：反向路径不再先构造双导轨。
//!
//! `dX[c] = Σ_r dy[r]·scale[r]·q[r,c]`，`qdot[r] = Σ_c q[r,c]·x[c]`；
//! 与 `PlaneMatrix::input_grad + row_dot` 的累加分组和顺序一致。

#[cfg(target_arch = "x86_64")]
use super::super::backward::has_avx2;

/// 返回 `(dX, qdot)`；`dy`、`scale` 长度是 `rows`，`x` 长度是 `cols`。
#[inline]
pub fn backward(
    latent: &[f32],
    scale: &[f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    dy: &[f32],
) -> (Vec<f32>, Vec<f32>) {
    assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
    assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    assert_eq!(dy.len(), rows, "dy 长度必须是 rows");
    #[cfg(target_arch = "x86_64")]
    if cols >= 8 && has_avx2() {
        // SAFETY: 长度已断言；内核按 8 列整块与尾部访问。
        return unsafe { simd::backward(latent, scale, rows, cols, x, dy) };
    }
    backward_scalar(latent, scale, rows, cols, x, dy)
}

fn backward_scalar(
    latent: &[f32],
    scale: &[f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    dy: &[f32],
) -> (Vec<f32>, Vec<f32>) {
    let mut dx = vec![0.0f32; cols];
    let mut qdot = vec![0.0f32; rows];
    for (r, slot) in qdot.iter_mut().enumerate() {
        let s = scale[r].max(f32::MIN_POSITIVE);
        let coeff = dy[r] * scale[r];
        for c in 0..cols {
            let q = (latent[r * cols + c] / s).round().clamp(-1.0, 1.0);
            *slot += q * x[c];
            dx[c] += coeff * q;
        }
    }
    (dx, qdot)
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
    pub unsafe fn backward(
        latent: &[f32],
        scale: &[f32],
        rows: usize,
        cols: usize,
        x: &[f32],
        dy: &[f32],
    ) -> (Vec<f32>, Vec<f32>) {
        let full = cols / 8;
        let mut acc: [Vec<f32>; 4] = std::array::from_fn(|_| vec![0.0f32; cols]);
        let mut qdot = vec![0.0f32; rows];
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_ps(1.0);
        let neg_one = _mm256_set1_ps(-1.0);
        let half = _mm256_set1_ps(0.5);
        let sign = _mm256_set1_ps(-0.0);
        for (r, slot) in qdot.iter_mut().enumerate() {
            let s = scale[r].max(f32::MIN_POSITIVE);
            let sv = _mm256_set1_ps(s);
            let coeff = dy[r] * scale[r];
            let cv = _mm256_set1_ps(coeff);
            let row = &latent[r * cols..(r + 1) * cols];
            let a = &mut acc[r & 3];
            let mut dacc = [_mm256_setzero_ps(); 4];
            let mut dot = |j: usize| -> __m256 {
                let base = j * 8;
                let wv = _mm256_loadu_ps(row.as_ptr().add(base));
                let scaled = _mm256_div_ps(wv, sv);
                let signed_half = _mm256_or_ps(_mm256_and_ps(scaled, sign), half);
                let rounded = _mm256_round_ps::<{ _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC }>(
                    _mm256_add_ps(scaled, signed_half),
                );
                let clamped = _mm256_min_ps(one, _mm256_max_ps(neg_one, rounded));
                let qi = _mm256_cvtps_epi32(clamped);
                let p = _mm256_cmpgt_epi32(qi, zero);
                let n = _mm256_cmpgt_epi32(zero, qi);
                let xv = _mm256_loadu_ps(x.as_ptr().add(base));
                let qx = _mm256_sub_ps(
                    _mm256_and_ps(xv, _mm256_castsi256_ps(p)),
                    _mm256_and_ps(xv, _mm256_castsi256_ps(n)),
                );
                let mut v = _mm256_loadu_ps(a.as_ptr().add(base));
                v = _mm256_add_ps(v, _mm256_and_ps(cv, _mm256_castsi256_ps(p)));
                v = _mm256_sub_ps(v, _mm256_and_ps(cv, _mm256_castsi256_ps(n)));
                _mm256_storeu_ps(a.as_mut_ptr().add(base), v);
                qx
            };
            let mut j = 0;
            while j + 4 <= full {
                dacc[0] = _mm256_add_ps(dacc[0], dot(j));
                dacc[1] = _mm256_add_ps(dacc[1], dot(j + 1));
                dacc[2] = _mm256_add_ps(dacc[2], dot(j + 2));
                dacc[3] = _mm256_add_ps(dacc[3], dot(j + 3));
                j += 4;
            }
            while j < full {
                dacc[0] = _mm256_add_ps(dacc[0], dot(j));
                j += 1;
            }
            *slot = hsum(_mm256_add_ps(
                _mm256_add_ps(dacc[0], dacc[1]),
                _mm256_add_ps(dacc[2], dacc[3]),
            ));
            for c in full * 8..cols {
                let q = (row[c] / s).round().clamp(-1.0, 1.0);
                *slot += q * x[c];
                if q > 0.0 {
                    a[c] += coeff;
                } else if q < 0.0 {
                    a[c] -= coeff;
                }
            }
        }
        let mut dx = vec![0.0f32; cols];
        for (c, slot) in dx.iter_mut().enumerate() {
            *slot = acc[0][c] + acc[1][c] + acc[2][c] + acc[3][c];
        }
        (dx, qdot)
    }
}

#[cfg(test)]
mod tests;
