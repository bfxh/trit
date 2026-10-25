//! AVX2/FMA 内核。
//!
//! 三值的两条主线在这里落地：
//! - [`row_dot_mask`]：双导轨 × f32，用 8 位掩码表把 `x * {-1,0,+1}` 变成
//!   `AND` + 加减，**不做任何浮点乘法**。
//! - [`Rail::dot`](crate::pack::Rail::dot)：双导轨 × 双导轨，4 组 `AND + popcount`。

/// 8 位掩码 -> 8 个 `i32`（0 或 -1）的查表。
#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "x86_64")]
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
        // 4 路独立累加器。单个 acc 会把每 64 trit 的 16 次 add/sub 串成一条依赖链
        // （AVX 加法延迟约 4 周期 → 下限约 64 周期/64 trit），把内核锁在 ALU 上。
        // 拆成 4 路后每路链长 4，瓶颈回到访存。
        //
        // 代价：浮点求和顺序改变，末位可能有 ulp 级差异；所有调用方按 1e-4 容差比较
        // （见 gemv 的 `matches_scalar` / `row_dot_matches_gemv`）。
        macro_rules! step {
            ($acc:ident, $k:expr, $p:expr, $n:expr, $base:expr) => {{
                let pb = (($p >> ($k * 8)) & 0xFF) as usize;
                let nb = (($n >> ($k * 8)) & 0xFF) as usize;
                // SAFETY: x 已补齐到 64 的倍数。
                let xv = _mm256_loadu_ps(x.as_ptr().add($base + $k * 8));
                let mp = _mm256_loadu_si256(MASK_LUT[pb].as_ptr() as *const __m256i);
                let mn = _mm256_loadu_si256(MASK_LUT[nb].as_ptr() as *const __m256i);
                $acc = _mm256_add_ps($acc, _mm256_and_ps(xv, _mm256_castsi256_ps(mp)));
                $acc = _mm256_sub_ps($acc, _mm256_and_ps(xv, _mm256_castsi256_ps(mn)));
            }};
        }
        let mut a0 = _mm256_setzero_ps();
        let mut a1 = _mm256_setzero_ps();
        let mut a2 = _mm256_setzero_ps();
        let mut a3 = _mm256_setzero_ps();
        for (b, (&p, &n)) in pos.iter().zip(neg).enumerate() {
            let base = b * 64;
            step!(a0, 0, p, n, base);
            step!(a1, 1, p, n, base);
            step!(a2, 2, p, n, base);
            step!(a3, 3, p, n, base);
            step!(a0, 4, p, n, base);
            step!(a1, 5, p, n, base);
            step!(a2, 6, p, n, base);
            step!(a3, 7, p, n, base);
        }
        hsum(_mm256_add_ps(_mm256_add_ps(a0, a1), _mm256_add_ps(a2, a3)))
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

/// 非 x86_64 的标量回退。
///
/// 语义与 `imp` 逐项一致（`row_dot_mask` 的按位加减、`row_dot_q4` 的 nibble 顺序
/// 都与 AVX2 版相同），只是没有向量化。x86_64 上也有测试拿它和 AVX2 版对拍，
/// 所以这里的正确性不依赖「在 ARM 上跑一遍」。
#[cfg_attr(target_arch = "x86_64", allow(dead_code))]
mod scalar {
    /// f16 -> f32（IEEE 754 binary16 → binary32）。
    fn f16_to_f32(h: u16) -> f32 {
        let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exp = ((h >> 10) & 0x1f) as i32;
        let frac = (h & 0x3ff) as f32;
        let v = match exp {
            0 => frac * 2f32.powi(-24),
            31 if frac == 0.0 => f32::INFINITY,
            31 => f32::NAN,
            _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
        };
        sign * v
    }

    /// 双导轨行 × f32 激活（标量）。
    ///
    /// # Safety
    /// `x` 长度至少 `pos.len() * 64`；调用方负责补零。
    pub unsafe fn row_dot_mask(pos: &[u64], neg: &[u64], x: &[f32]) -> f32 {
        let mut acc = 0.0f32;
        for (b, (&p, &n)) in pos.iter().zip(neg).enumerate() {
            let base = b * 64;
            for k in 0..64 {
                let bit = 1u64 << k;
                if p & bit != 0 {
                    acc += x[base + k];
                } else if n & bit != 0 {
                    acc -= x[base + k];
                }
            }
        }
        acc
    }

    /// f32 行 × f32 激活（标量基线）。
    ///
    /// # Safety
    /// `w` 与 `x` 等长。
    pub unsafe fn row_dot_f32(w: &[f32], x: &[f32]) -> f32 {
        w.iter().zip(x).map(|(a, b)| a * b).sum()
    }

    /// f16 行 × f32 激活（标量基线）。
    ///
    /// # Safety
    /// `w` 与 `x` 等长。
    pub unsafe fn row_dot_f16(w: &[u16], x: &[f32]) -> f32 {
        w.iter().zip(x).map(|(&h, &xv)| f16_to_f32(h) * xv).sum()
    }

    /// Q4_0 行 × Q8 激活（标量基线，整数点积）。
    ///
    /// 每块 18 字节：2 字节 f16 scale + 16 字节 nibble；激活每 32 个一个 scale。
    ///
    /// # Safety
    /// `w` 覆盖 `dx.len()` 个块；`qx` 至少 `dx.len() * 32`。
    pub unsafe fn row_dot_q4(w: &[u8], qx: &[i8], dx: &[f32]) -> f32 {
        let mut acc = 0.0f32;
        for (b, &d_act) in dx.iter().enumerate() {
            let base = b * 18;
            let dw = f16_to_f32(u16::from_le_bytes([w[base], w[base + 1]]));
            let mut dot = 0i32;
            for i in 0..16 {
                let byte = w[base + 2 + i] as i32;
                let lo = (byte & 0x0f) - 8;
                let hi = ((byte >> 4) & 0x0f) - 8;
                dot += qx[b * 32 + i] as i32 * lo;
                dot += qx[b * 32 + 16 + i] as i32 * hi;
            }
            acc += dot as f32 * (dw * d_act);
        }
        acc
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub use scalar::{row_dot_f16, row_dot_f32, row_dot_mask, row_dot_q4};

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

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;

    fn rails(rows: usize, cols: usize) -> (Vec<u64>, Vec<u64>, Vec<f32>) {
        let bpr = cols.div_ceil(64);
        let padded = bpr * 64;
        let mut pos = vec![0u64; rows * bpr];
        let mut neg = vec![0u64; rows * bpr];
        for r in 0..rows {
            for c in 0..cols {
                let t = ((r * 31 + c * 17) % 3) as i32 - 1;
                let word = r * bpr + c / 64;
                let bit = 1u64 << (c % 64);
                match t {
                    1 => pos[word] |= bit,
                    -1 => neg[word] |= bit,
                    _ => {}
                }
            }
        }
        let x = (0..padded)
            .map(|i| ((i as f32 * 0.37).sin() * 0.5) + ((i % 7) as f32 - 3.0) * 0.01)
            .collect();
        (pos, neg, x)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-3 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn scalar_mask_matches_avx2() {
        if !avx2() {
            return;
        }
        let (rows, cols) = (9usize, 576usize);
        let (pos, neg, x) = rails(rows, cols);
        let bpr = cols.div_ceil(64);
        for r in 0..rows {
            let (p, n) = (&pos[r * bpr..(r + 1) * bpr], &neg[r * bpr..(r + 1) * bpr]);
            // SAFETY: x 已补齐到 bpr*64。
            let (fast, slow) =
                unsafe { (imp::row_dot_mask(p, n, &x), scalar::row_dot_mask(p, n, &x)) };
            assert!(close(fast, slow), "mask row {r}: avx2={fast} scalar={slow}");
        }
    }

    #[test]
    fn scalar_f32_and_f16_match_avx2() {
        if !avx2() {
            return;
        }
        let n = 576usize;
        let w: Vec<f32> = (0..n).map(|i| ((i * 13 % 17) as f32 - 8.0) * 0.1).collect();
        let x: Vec<f32> = (0..n).map(|i| ((i * 7 % 11) as f32 - 5.0) * 0.05).collect();
        // SAFETY: 等长切片。
        let (fast, slow) = unsafe { (imp::row_dot_f32(&w, &x), scalar::row_dot_f32(&w, &x)) };
        assert!(close(fast, slow), "f32: avx2={fast} scalar={slow}");

        let h: Vec<u16> = w.iter().map(|&v| crate::bench::to_f16(v)).collect();
        // SAFETY: 等长切片。
        let (fast, slow) = unsafe { (imp::row_dot_f16(&h, &x), scalar::row_dot_f16(&h, &x)) };
        assert!(close(fast, slow), "f16: avx2={fast} scalar={slow}");
    }

    #[test]
    fn scalar_q4_matches_avx2() {
        if !avx2() {
            return;
        }
        let cols = 64usize;
        let w: Vec<f32> = (0..cols)
            .map(|i| ((i * 5 % 13) as f32 - 6.0) * 0.2)
            .collect();
        let packed = crate::bench::pack_q4(&w);
        let x: Vec<f32> = (0..cols)
            .map(|i| ((i * 3 % 9) as f32 - 4.0) * 0.1)
            .collect();
        let (qx, dx) = quantize_act_q8(&x);
        // SAFETY: packed 覆盖 dx.len() 个块，qx 覆盖 32*块数。
        let (fast, slow) = unsafe {
            (
                imp::row_dot_q4(&packed, &qx, &dx),
                scalar::row_dot_q4(&packed, &qx, &dx),
            )
        };
        assert!(close(fast, slow), "q4: avx2={fast} scalar={slow}");
    }
}
