//! AVX2/FMA 内核。
//!
//! 三值的两条主线在这里落地：
//! - [`row_dot_mask`]：双导轨 × f32，用 8 位掩码表把 `x * {-1,0,+1}` 变成
//!   `AND` + 加减，**不做任何浮点乘法**。
//! - [`row_dot_rail`]：双导轨 × 双导轨，4 组 `AND + popcount`。

/// 8 位掩码 -> 8 个 `i32`（0 或 -1）的查表。
const fn build_mask_lut() -> [[i32; 8]; 256] {
    let mut table = [[0i32; 8]; 256];
    let mut b = 0usize;
    while b < 256 {
        let mut k = 0usize;
        while k < 8 {
            table[b][k] = if (b >> k) & 1 == 1 { -1 } else { 0 };
            k += 1;
        }
        b += 1;
    }
    table
}

static MASK_LUT: [[i32; 8]; 256] = build_mask_lut();

/// 本机是否支持 AVX2 + FMA。
pub fn avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use std::sync::OnceLock;
        static OK: OnceLock<bool> = OnceLock::new();
        *OK.get_or_init(|| {
            std::arch::is_x86_feature_detected!("avx2")
                && std::arch::is_x86_feature_detected!("fma")
        })
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

#[cfg(target_arch = "x86_64")]
mod imp {
    use super::MASK_LUT;
    use std::arch::x86_64::*;

    /// 256 位寄存器求和。
    ///
    /// # Safety
    /// 需要 AVX。
    #[inline]
    #[target_feature(enable = "avx")]
    pub unsafe fn hsum(v: __m256) -> f32 {
        let lo = _mm256_castps256_ps128(v);
        let hi = _mm256_extractf128_ps(v, 1);
        let s = _mm_add_ps(lo, hi);
        let s = _mm_hadd_ps(s, s);
        let s = _mm_hadd_ps(s, s);
        _mm_cvtss_f32(s)
    }

    /// f16 -> f32（F16C，无分支）。
    ///
    /// # Safety
    /// 需要 F16C。
    #[inline]
    #[target_feature(enable = "f16c")]
    pub unsafe fn f16_to_f32(h: u16) -> f32 {
        _mm_cvtss_f32(_mm_cvtph_ps(_mm_cvtsi32_si128(h as i32)))
    }

    /// 双导轨行 × f32 激活：按位掩码累加，无乘法。
    ///
    /// `x` 必须补齐到 64 的倍数（调用方负责补零），长度覆盖整行。
    ///
    /// # Safety
    /// 需要 AVX2 + FMA；`x` 长度至少 `pos.len() * 64`。
    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn row_dot_mask(pos: &[u64], neg: &[u64], x: &[f32]) -> f32 {
        let mut acc = _mm256_setzero_ps();
        for (b, (&p, &n)) in pos.iter().zip(neg).enumerate() {
            let base = b * 64;
            for k in 0..8 {
                let pb = ((p >> (k * 8)) & 0xFF) as usize;
                let nb = ((n >> (k * 8)) & 0xFF) as usize;
                // SAFETY: x 已补齐到 64 的倍数。
                let xv = _mm256_loadu_ps(x.as_ptr().add(base + k * 8));
                let mp = _mm256_loadu_si256(MASK_LUT[pb].as_ptr() as *const __m256i);
                let mn = _mm256_loadu_si256(MASK_LUT[nb].as_ptr() as *const __m256i);
                acc = _mm256_add_ps(acc, _mm256_and_ps(xv, _mm256_castsi256_ps(mp)));
                acc = _mm256_sub_ps(acc, _mm256_and_ps(xv, _mm256_castsi256_ps(mn)));
            }
        }
        hsum(acc)
    }

    /// f32 行 × f32 激活（基线）。
    ///
    /// # Safety
    /// 需要 AVX2 + FMA。
    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn row_dot_f32(w: &[f32], x: &[f32]) -> f32 {
        let mut a0 = _mm256_setzero_ps();
        let mut a1 = _mm256_setzero_ps();
        let mut a2 = _mm256_setzero_ps();
        let mut a3 = _mm256_setzero_ps();
        let n = w.len();
        let mut i = 0usize;
        while i + 32 <= n {
            a0 = _mm256_fmadd_ps(
                _mm256_loadu_ps(w.as_ptr().add(i)),
                _mm256_loadu_ps(x.as_ptr().add(i)),
                a0,
            );
            a1 = _mm256_fmadd_ps(
                _mm256_loadu_ps(w.as_ptr().add(i + 8)),
                _mm256_loadu_ps(x.as_ptr().add(i + 8)),
                a1,
            );
            a2 = _mm256_fmadd_ps(
                _mm256_loadu_ps(w.as_ptr().add(i + 16)),
                _mm256_loadu_ps(x.as_ptr().add(i + 16)),
                a2,
            );
            a3 = _mm256_fmadd_ps(
                _mm256_loadu_ps(w.as_ptr().add(i + 24)),
                _mm256_loadu_ps(x.as_ptr().add(i + 24)),
                a3,
            );
            i += 32;
        }
        let mut s = hsum(_mm256_add_ps(_mm256_add_ps(a0, a1), _mm256_add_ps(a2, a3)));
        while i < n {
            s += *w.get_unchecked(i) * *x.get_unchecked(i);
            i += 1;
        }
        s
    }

    /// f16 行 × f32 激活（基线）。
    ///
    /// # Safety
    /// 需要 AVX2 + FMA + F16C。
    #[target_feature(enable = "avx2,fma,f16c")]
    pub unsafe fn row_dot_f16(w: &[u16], x: &[f32]) -> f32 {
        let n = w.len();
        let mut a0 = _mm256_setzero_ps();
        let mut a1 = _mm256_setzero_ps();
        let mut i = 0usize;
        while i + 16 <= n {
            let w0 = _mm256_cvtph_ps(_mm_loadu_si128(w.as_ptr().add(i) as *const __m128i));
            let w1 = _mm256_cvtph_ps(_mm_loadu_si128(w.as_ptr().add(i + 8) as *const __m128i));
            a0 = _mm256_fmadd_ps(w0, _mm256_loadu_ps(x.as_ptr().add(i)), a0);
            a1 = _mm256_fmadd_ps(w1, _mm256_loadu_ps(x.as_ptr().add(i + 8)), a1);
            i += 16;
        }
        let mut s = hsum(_mm256_add_ps(a0, a1));
        while i < n {
            s += f16_to_f32(*w.get_unchecked(i)) * *x.get_unchecked(i);
            i += 1;
        }
        s
    }

    /// Q4_0 行 × Q8 激活（基线，整数点积）。
    ///
    /// 每块 18 字节：2 字节 f16 scale + 16 字节 nibble。
    ///
    /// # Safety
    /// 需要 AVX2 + FMA + F16C；`qx` / `dx` 长度覆盖整行。
    #[target_feature(enable = "avx2,fma,f16c")]
    pub unsafe fn row_dot_q4(w: &[u8], qx: &[i8], dx: &[f32]) -> f32 {
        let nblocks = w.len() / 18;
        let ones = _mm256_set1_epi16(1);
        let m4 = _mm_set1_epi8(0x0f);
        let e8 = _mm_set1_epi8(8);
        let mut facc = _mm256_setzero_ps();
        for (b, &d_act) in dx.iter().enumerate().take(nblocks) {
            let base = b * 18;
            let dw = f16_to_f32(u16::from_le_bytes([w[base], w[base + 1]]));
            let raw = _mm_loadu_si128(w.as_ptr().add(base + 2) as *const __m128i);
            let lo = _mm_sub_epi8(_mm_and_si128(raw, m4), e8);
            let hi = _mm_sub_epi8(_mm_and_si128(_mm_srli_epi16(raw, 4), m4), e8);
            let qw = _mm256_inserti128_si256(_mm256_castsi128_si256(lo), hi, 1);
            let qv = _mm256_loadu_si256(qx.as_ptr().add(b * 32) as *const __m256i);
            let p16 = _mm256_maddubs_epi16(_mm256_sign_epi8(qv, qv), _mm256_sign_epi8(qw, qv));
            let s32 = _mm256_madd_epi16(p16, ones);
            facc = _mm256_fmadd_ps(_mm256_cvtepi32_ps(s32), _mm256_set1_ps(dw * d_act), facc);
        }
        hsum(facc)
    }
}

#[cfg(target_arch = "x86_64")]
pub use imp::{f16_to_f32, hsum, row_dot_f16, row_dot_f32, row_dot_mask, row_dot_q4};

/// 把 f32 激活量化成 Q8（每 32 元素一个 scale），供 Q4 基线使用。
pub fn quantize_act_q8(x: &[f32]) -> (Vec<i8>, Vec<f32>) {
    let nb = x.len() / 32;
    let mut qs = vec![0i8; nb * 32];
    let mut ds = vec![0.0f32; nb];
    for b in 0..nb {
        let seg = &x[b * 32..(b + 1) * 32];
        let amax = seg.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let d = if amax > 0.0 { amax / 127.0 } else { 0.0 };
        ds[b] = d;
        let inv = if d > 0.0 { 1.0 / d } else { 0.0 };
        for i in 0..32 {
            qs[b * 32 + i] = (seg[i] * inv).round().clamp(-127.0, 127.0) as i8;
        }
    }
    (qs, ds)
}
