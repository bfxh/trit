//! `PlaneMatrix` 的量化打包：从 latent + per-row scale 一步生成双导轨；`q` 惰性解码。
//!
//! 这是训练步的入口热点：`pack()` 每步跑一次。`from_latent` 在 x86_64/AVX2 上
//! 每 8 列做一次除法、round、clamp，并用 `movemask` 直接写一个字节的
//! `pos` / `neg` 掩码；非 x86 或列数 < 8 走标量参考。

#[cfg(target_arch = "x86_64")]
use super::backward::has_avx2;
use super::{PlaneMatrix, TRITS_PER_WORD};
impl PlaneMatrix {
    /// 从 latent + 每行 scale 一步量化并打包双导轨；`q` 首次 `q()` 时解码。
    ///
    /// 语义与 `quantize_with_scale(row, scale[r])` 完全一致：
    /// `q = clamp(round(w / scale), -1, 1)`，`scale` 先抬到 `f32::MIN_POSITIVE`。
    #[inline]
    pub fn from_latent(latent: &[f32], scale: &[f32], rows: usize, cols: usize) -> Self {
        assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
        assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
        #[cfg(target_arch = "x86_64")]
        if cols >= 8 && has_avx2() {
            // SAFETY: 长度已断言；内核按 rows/cols 与 8 列整块访问，尾部单独补齐。
            return unsafe { simd::from_latent(latent, scale, rows, cols) };
        }
        Self::from_latent_scalar(latent, scale, rows, cols)
    }

    fn from_latent_scalar(latent: &[f32], scale: &[f32], rows: usize, cols: usize) -> Self {
        let bpr = cols.div_ceil(TRITS_PER_WORD).max(1);
        let mut pos = vec![0u64; rows * bpr];
        let mut neg = vec![0u64; rows * bpr];
        for (r, &s) in scale.iter().take(rows).enumerate() {
            let s = s.max(f32::MIN_POSITIVE);
            for c in 0..cols {
                let qi = (latent[r * cols + c] / s).round().clamp(-1.0, 1.0) as i8;
                let word = r * bpr + c / TRITS_PER_WORD;
                let bit = 1u64 << (c % TRITS_PER_WORD);
                match qi {
                    1 => pos[word] |= bit,
                    -1 => neg[word] |= bit,
                    _ => {}
                }
            }
        }
        Self {
            rows,
            cols,
            bpr,
            q: std::sync::OnceLock::new(),
            pos,
            neg,
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod simd {
    #![allow(unsafe_op_in_unsafe_fn)]
    use std::arch::x86_64::*;

    use super::PlaneMatrix;

    /// 从 latent + per-row scale 一步量化并打包：每 8 列一次除法、round、clamp，
    /// 用 `movemask` 直接写一个字节的 `pos` / `neg` 掩码。
    ///
    /// # Safety
    /// 需要 AVX2；`latent.len() == rows * cols`，`scale.len() == rows`。
    #[target_feature(enable = "avx2")]
    pub unsafe fn from_latent(
        latent: &[f32],
        scale: &[f32],
        rows: usize,
        cols: usize,
    ) -> PlaneMatrix {
        let bpr = cols.div_ceil(64).max(1);
        let mut pos = vec![0u64; rows * bpr];
        let mut neg = vec![0u64; rows * bpr];
        let sign = _mm256_set1_ps(-0.0);
        let half = _mm256_set1_ps(0.5);
        let one = _mm256_set1_ps(1.0);
        let neg_one = _mm256_set1_ps(-1.0);
        let zero_i = _mm256_setzero_si256();
        let full = cols / 8;
        for (r, &s) in scale.iter().take(rows).enumerate() {
            let s = s.max(f32::MIN_POSITIVE);
            let sv = _mm256_set1_ps(s);
            let row_base = r * cols;
            for j in 0..full {
                let base = row_base + j * 8;
                // SAFETY: base + 8 <= row_base + full * 8 <= latent.len()。
                let w = _mm256_loadu_ps(latent.as_ptr().add(base));
                let scaled = _mm256_div_ps(w, sv);
                // Rust 的 round 是“半远离零”；SIMD 用 trunc(x + copysign(0.5, x)) 复刻。
                let signed_half = _mm256_or_ps(_mm256_and_ps(scaled, sign), half);
                let rounded = _mm256_round_ps::<{ _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC }>(
                    _mm256_add_ps(scaled, signed_half),
                );
                let clamped = _mm256_min_ps(one, _mm256_max_ps(neg_one, rounded));
                let qi = _mm256_cvtps_epi32(clamped);
                let pm = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpgt_epi32(qi, zero_i)));
                let nm = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpgt_epi32(zero_i, qi)));
                let word = r * bpr + (j * 8) / 64;
                let shift = ((j * 8) % 64) as u32;
                pos[word] |= (pm as u64) << shift;
                neg[word] |= (nm as u64) << shift;
            }
            for c in full * 8..cols {
                let qi = (latent[row_base + c] / s).round().clamp(-1.0, 1.0) as i8;
                let word = r * bpr + c / 64;
                let bit = 1u64 << (c % 64);
                match qi {
                    1 => pos[word] |= bit,
                    -1 => neg[word] |= bit,
                    _ => {}
                }
            }
        }
        PlaneMatrix {
            rows,
            cols,
            bpr,
            q: std::sync::OnceLock::new(),
            pos,
            neg,
        }
    }
}

impl PartialEq for PlaneMatrix {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows
            && self.cols == other.cols
            && self.pos == other.pos
            && self.neg == other.neg
    }
}
impl Eq for PlaneMatrix {}

#[cfg(test)]
mod tests;
