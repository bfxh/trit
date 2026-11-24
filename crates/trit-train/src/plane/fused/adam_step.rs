//! Adam 训练步单遍：量化 `q`、`dX`、`qdot` 与行级 `dW` 更新共用一次
//! latent/m/v 读取。
//!
//! `dX[c] = Σ_r dy[r]·scale[r]·q[r,c]`、`qdot[r] = Σ_c q[r,c]·x[c]`；
//! `dX` 的 `r & 3` 分组、`qdot` 的 `j mod 4` 分组和 Adam 更新算术都与旧
//! `fused::backward + layer::adam_update` 逐位一致。scale 仍由调用方在行外更新。

use crate::quant::Ste;

/// Adam 的超参数快照；由 `layer::Adam` 在 `begin()` 之后构造。
#[derive(Clone, Copy)]
pub struct AdamParams {
    pub lr: f32,
    pub beta1: f32,
    pub beta2: f32,
    pub eps: f32,
    pub bias1: f32,
    pub bias2: f32,
}

impl AdamParams {
    /// 记录一次 Adam 步的系数。
    pub fn new(lr: f32, beta1: f32, beta2: f32, eps: f32, bias1: f32, bias2: f32) -> Self {
        Self {
            lr,
            beta1,
            beta2,
            eps,
            bias1,
            bias2,
        }
    }
}

#[cfg(target_arch = "x86_64")]
use super::super::backward::has_avx2;

/// 对整层 latent 做一次 Adam 更新，同时返回 `(dX, qdot)`。
///
/// `scale` 是更新前的每行 scale；调用方用返回的 `qdot` 更新 scale。
/// `m`/`v` 是 Adam 一阶/二阶矩，长度均为 `rows * cols`。
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn adam_step(
    latent: &mut [f32],
    scale: &[f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    dy: &[f32],
    m: &mut [f32],
    v: &mut [f32],
    params: AdamParams,
    ste: Ste,
) -> (Vec<f32>, Vec<f32>) {
    assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
    assert_eq!(m.len(), rows * cols, "m 长度必须是 rows * cols");
    assert_eq!(v.len(), rows * cols, "v 长度必须是 rows * cols");
    assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    assert_eq!(dy.len(), rows, "dy 长度必须是 rows");
    #[cfg(target_arch = "x86_64")]
    if cols >= 8 && has_avx2() {
        // SAFETY: 长度已断言；内核只按 8 列整块与尾部访问。
        return unsafe { simd::adam_step(latent, scale, rows, cols, x, dy, m, v, params, ste) };
    }
    adam_step_scalar(latent, scale, rows, cols, x, dy, m, v, params, ste)
}

#[allow(clippy::too_many_arguments)]
fn adam_step_scalar(
    latent: &mut [f32],
    scale: &[f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    dy: &[f32],
    m: &mut [f32],
    v: &mut [f32],
    params: AdamParams,
    ste: Ste,
) -> (Vec<f32>, Vec<f32>) {
    let mut dx = vec![0.0f32; cols];
    let mut qdot = vec![0.0f32; rows];
    for r in 0..rows {
        let s_orig = scale[r];
        let s = s_orig.max(f32::MIN_POSITIVE);
        let coeff = dy[r] * s_orig;
        let base = r * cols;
        let row = &mut latent[base..base + cols];
        let mrow = &mut m[base..base + cols];
        let vrow = &mut v[base..base + cols];
        for c in 0..cols {
            let w = row[c];
            let q = (w / s).round().clamp(-1.0, 1.0);
            if q > 0.0 {
                qdot[r] += x[c];
                dx[c] += coeff;
            } else if q < 0.0 {
                qdot[r] -= x[c];
                dx[c] -= coeff;
            }
            let grad = ste.backward(w, s_orig, coeff * x[c]);
            mrow[c] = params.beta1 * mrow[c] + (1.0 - params.beta1) * grad;
            vrow[c] = params.beta2 * vrow[c] + (1.0 - params.beta2) * grad * grad;
            let m_hat = mrow[c] / params.bias1;
            let v_hat = vrow[c] / params.bias2;
            row[c] = w - params.lr * m_hat / (v_hat.sqrt() + params.eps);
        }
    }
    (dx, qdot)
}

#[cfg(target_arch = "x86_64")]
mod simd {
    #![allow(unsafe_op_in_unsafe_fn)]
    use std::arch::x86_64::*;

    use super::AdamParams;
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

    #[allow(clippy::too_many_arguments)]
    #[target_feature(enable = "avx2")]
    pub unsafe fn adam_step(
        latent: &mut [f32],
        scale: &[f32],
        rows: usize,
        cols: usize,
        x: &[f32],
        dy: &[f32],
        m: &mut [f32],
        v: &mut [f32],
        params: AdamParams,
        ste: Ste,
    ) -> (Vec<f32>, Vec<f32>) {
        let full = cols / 8;
        let mut acc: [Vec<f32>; 4] = std::array::from_fn(|_| vec![0.0f32; cols]);
        let mut qdot = vec![0.0f32; rows];
        let izero = _mm256_setzero_si256();
        let one = _mm256_set1_ps(1.0);
        let neg_one = _mm256_set1_ps(-1.0);
        let half = _mm256_set1_ps(0.5);
        let sign = _mm256_set1_ps(-0.0);
        let zero = _mm256_setzero_ps();
        let b1 = _mm256_set1_ps(params.beta1);
        let b2 = _mm256_set1_ps(params.beta2);
        let one_b1 = _mm256_set1_ps(1.0 - params.beta1);
        let one_b2 = _mm256_set1_ps(1.0 - params.beta2);
        let bias1 = _mm256_set1_ps(params.bias1);
        let bias2 = _mm256_set1_ps(params.bias2);
        let lrv = _mm256_set1_ps(params.lr);
        let eps = _mm256_set1_ps(params.eps);
        for r in 0..rows {
            let s_orig = scale[r];
            let s = s_orig.max(f32::MIN_POSITIVE);
            let sv = _mm256_set1_ps(s);
            let iv = _mm256_set1_ps(1.0 / s);
            let coeff = dy[r] * s_orig;
            let cv = _mm256_set1_ps(coeff);
            let base = r * cols;
            let row = &mut latent[base..base + cols];
            let mrow = &mut m[base..base + cols];
            let vrow = &mut v[base..base + cols];
            let a = &mut acc[r & 3];
            let mut dacc = [_mm256_setzero_ps(); 4];
            let mut block = |j: usize| -> __m256 {
                let off = j * 8;
                let wv = _mm256_loadu_ps(row.as_ptr().add(off));
                let xv = _mm256_loadu_ps(x.as_ptr().add(off));
                let scaled = _mm256_div_ps(wv, sv);
                let signed_half = _mm256_or_ps(_mm256_and_ps(scaled, sign), half);
                let rounded = _mm256_round_ps::<{ _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC }>(
                    _mm256_add_ps(scaled, signed_half),
                );
                let clamped = _mm256_min_ps(one, _mm256_max_ps(neg_one, rounded));
                let qi = _mm256_cvtps_epi32(clamped);
                let p = _mm256_cmpgt_epi32(qi, izero);
                let n = _mm256_cmpgt_epi32(izero, qi);
                let qx = _mm256_sub_ps(
                    _mm256_and_ps(xv, _mm256_castsi256_ps(p)),
                    _mm256_and_ps(xv, _mm256_castsi256_ps(n)),
                );
                let mut dv = _mm256_loadu_ps(a.as_ptr().add(off));
                dv = _mm256_add_ps(dv, _mm256_and_ps(cv, _mm256_castsi256_ps(p)));
                dv = _mm256_sub_ps(dv, _mm256_and_ps(cv, _mm256_castsi256_ps(n)));
                _mm256_storeu_ps(a.as_mut_ptr().add(off), dv);

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
                        let g = _mm256_max_ps(zero, _mm256_sub_ps(one, sq));
                        let ordered = _mm256_cmp_ps(ratio, ratio, _CMP_ORD_Q);
                        _mm256_and_ps(_mm256_mul_ps(base_grad, g), ordered)
                    }
                };
                let m_old = _mm256_loadu_ps(mrow.as_ptr().add(off));
                let v_old = _mm256_loadu_ps(vrow.as_ptr().add(off));
                let m_new = _mm256_add_ps(_mm256_mul_ps(b1, m_old), _mm256_mul_ps(one_b1, grad));
                let v_new = _mm256_add_ps(
                    _mm256_mul_ps(b2, v_old),
                    _mm256_mul_ps(one_b2, _mm256_mul_ps(grad, grad)),
                );
                let m_hat = _mm256_div_ps(m_new, bias1);
                let v_hat = _mm256_div_ps(v_new, bias2);
                let denom = _mm256_add_ps(_mm256_sqrt_ps(v_hat), eps);
                let next = _mm256_sub_ps(wv, _mm256_div_ps(_mm256_mul_ps(lrv, m_hat), denom));
                _mm256_storeu_ps(row.as_mut_ptr().add(off), next);
                _mm256_storeu_ps(mrow.as_mut_ptr().add(off), m_new);
                _mm256_storeu_ps(vrow.as_mut_ptr().add(off), v_new);
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
                if q > 0.0 {
                    qdot[r] += x[c];
                    a[c] += coeff;
                } else if q < 0.0 {
                    qdot[r] -= x[c];
                    a[c] -= coeff;
                }
                let grad = ste.backward(w, s_orig, coeff * x[c]);
                let m_old = mrow[c];
                let v_old = vrow[c];
                mrow[c] = params.beta1 * m_old + (1.0 - params.beta1) * grad;
                vrow[c] = params.beta2 * v_old + (1.0 - params.beta2) * grad * grad;
                let m_hat = mrow[c] / params.bias1;
                let v_hat = vrow[c] / params.bias2;
                row[c] = w - params.lr * m_hat / (v_hat.sqrt() + params.eps);
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
