//! 逐行 `dW` 外积更新：`w[r,c] -= lr · STE(w[r,c], scale[r], dy[r]·scale[r]·x[c])`。
//!
//! 这是 `dW` 的融合更新，不是位平面运算；`dX` 的位平面内核在
//! [`crate::plane::PlaneMatrix`]。SIMD 路径只在 x86_64 + AVX2 + 列数 ≥ 8 时启用，
//! 其余走标量参考。

use crate::quant::Ste;
/// 对整层 latent 做一次 SGD 外积更新。
///
/// 计时/调用口径：`dy`、`scale` 长度是 `rows`，`x` 长度是 `cols`，
/// `latent` 是行主序 `rows * cols`；`scale[r]` 是更新前的值。
#[allow(clippy::too_many_arguments)]
pub(crate) fn sgd_update(
    latent: &mut [f32],
    x: &[f32],
    rows: usize,
    cols: usize,
    dy: &[f32],
    scale: &[f32],
    lr: f32,
    ste: Ste,
) {
    assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    assert_eq!(dy.len(), rows, "dy 长度必须是 rows");
    assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
    for r in 0..rows {
        let coeff = dy[r] * scale[r];
        let row = &mut latent[r * cols..(r + 1) * cols];
        #[cfg(target_arch = "x86_64")]
        if cols >= 8 && has_avx2() {
            // SAFETY: row.len() == cols，x.len() == cols；内核只按 8 的整块和尾部访问。
            unsafe {
                simd::update_row(row, x, coeff, lr, scale[r], ste);
            }
            continue;
        }
        update_row_scalar(row, x, coeff, lr, scale[r], ste);
    }
}

fn update_row_scalar(row: &mut [f32], x: &[f32], coeff: f32, lr: f32, scale: f32, ste: Ste) {
    for (w, &xc) in row.iter_mut().zip(x) {
        *w -= lr * ste.backward(*w, scale, coeff * xc);
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

    use crate::quant::Ste;

    /// 融合 `dW` 更新：Identity / Clipped / Soft 三种门都只做 8 lane 的
    /// `w - lr * coeff * x * gate`；尾部仍用 `Ste::backward` 标量补齐。
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
                    // 交换 max 的操作数：NaN 时 Intel max 返回第二个操作数 0，
                    // 与标量 `(1 - NaN).max(0.0)` 一致，省掉 ordered 掩码。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_f32(len: usize, seed: u64) -> Vec<f32> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state % 2001) as f32 - 1000.0) / 500.0
            })
            .collect()
    }

    fn scalar_reference(
        latent: &mut [f32],
        x: &[f32],
        rows: usize,
        dy: &[f32],
        scale: &[f32],
        lr: f32,
        ste: Ste,
    ) {
        let cols = x.len();
        for (r, (&dy_r, &s)) in dy.iter().zip(scale).take(rows).enumerate() {
            let coeff = dy_r * s;
            for (c, &xc) in x.iter().take(cols).enumerate() {
                let idx = r * cols + c;
                latent[idx] -= lr * ste.backward(latent[idx], s, coeff * xc);
            }
        }
    }

    #[test]
    fn sgd_update_matches_scalar_for_all_ste_variants() {
        for (rows, cols) in [(2usize, 7usize), (5, 9), (9, 65), (11, 130)] {
            for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
                let latent = sample_f32(rows * cols, 101 + (rows * 100 + cols) as u64);
                let x = sample_f32(cols, 103 + rows as u64);
                let dy = sample_f32(rows, 107 + cols as u64);
                let scale: Vec<f32> = sample_f32(rows, 109 + cols as u64)
                    .into_iter()
                    .map(|v| v.abs() + 0.01)
                    .collect();
                let mut fast = latent.clone();
                let mut slow = latent;
                sgd_update(&mut fast, &x, rows, cols, &dy, &scale, 0.05, ste);
                scalar_reference(&mut slow, &x, rows, &dy, &scale, 0.05, ste);
                for (i, (a, b)) in fast.iter().zip(&slow).enumerate() {
                    assert!(
                        (a - b).abs() <= 1e-4 * (1.0 + b.abs()),
                        "{ste:?} {rows}x{cols} idx={i}: fast={a} slow={b}"
                    );
                }
            }
        }
    }

    #[test]
    fn sgd_update_is_noop_for_zero_lr() {
        let latent = sample_f32(5 * 65, 211);
        let x = sample_f32(65, 223);
        let dy = sample_f32(5, 227);
        let scale = vec![0.5f32; 5];
        let mut got = latent.clone();
        sgd_update(&mut got, &x, 5, 65, &dy, &scale, 0.0, Ste::Soft);
        assert_eq!(got, latent);
    }
}
