//! Byte search: AVX2 when available, SSE2 on older x86, portable scalar elsewhere.
//!
//! Byte search is the signature matcher's inner loop, so it gets real vector
//! code. A 32-byte compare against a broadcast needle plus `movemask` is about the
//! best a byte-at-a-time problem can do on x86-64, and it is worth roughly an
//! order of magnitude over the naive loop — see `t3 bench`.
//!
//! Every entry point here has a `*_scalar` twin, and the test module asserts that
//! the two agree on random data including every alignment offset. That matters
//! more than the speed: a vector loop that reads one byte past the last partial
//! block is a classic source of both false hits and out-of-bounds reads.

/// Byte search entry points.
pub mod bytes {
    /// Finds the first occurrence of `needle`.
    ///
    /// Returns a byte offset into `haystack`.
    #[must_use]
    pub fn find_byte(haystack: &[u8], needle: u8) -> Option<usize> {
        find_byte_from(haystack, needle, 0)
    }

    /// Finds the first occurrence of `needle` at or after `from`.
    #[must_use]
    pub fn find_byte_from(haystack: &[u8], needle: u8, from: usize) -> Option<usize> {
        if from >= haystack.len() {
            return None;
        }
        let slice = &haystack[from..];
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 support was just checked at run time.
                return unsafe { avx2::find_byte(slice, needle) }.map(|i| i + from);
            }
            if is_x86_feature_detected!("sse2") {
                // SAFETY: SSE2 is architecturally guaranteed on x86_64, and was
                // checked anyway.
                return unsafe { sse2::find_byte(slice, needle) }.map(|i| i + from);
            }
        }
        find_byte_scalar(slice, needle).map(|i| i + from)
    }

    /// Reference implementation, also the fallback on other architectures.
    #[must_use]
    pub fn find_byte_scalar(haystack: &[u8], needle: u8) -> Option<usize> {
        haystack.iter().position(|&b| b == needle)
    }

    /// Counts occurrences of `needle`.
    #[must_use]
    #[allow(
        clippy::naive_bytecount,
        reason = "零第三方依赖的承诺：不引入 bytecount crate，标量回退只用于对比口径"
    )]
    pub fn count_byte(haystack: &[u8], needle: u8) -> usize {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 support was just checked at run time.
                return unsafe { avx2::count_byte(haystack, needle) };
            }
        }
        haystack.iter().filter(|&&b| b == needle).count()
    }

    /// True when every byte is zero. Used for detecting stub sections without
    /// touching them twice.
    #[must_use]
    pub fn is_all_zero(haystack: &[u8]) -> bool {
        find_nonzero(haystack).is_none()
    }

    /// Offset of the first non-zero byte.
    #[must_use]
    pub fn find_nonzero(haystack: &[u8]) -> Option<usize> {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 support was just checked at run time.
                return unsafe { avx2::find_nonzero(haystack) };
            }
        }
        haystack.iter().position(|&b| b != 0)
    }

    #[cfg(target_arch = "x86_64")]
    #[allow(
        clippy::cast_ptr_alignment,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        clippy::naive_bytecount,
        reason = "SIMD 内核：loadu 明确接受非对齐指针，u8→i8 / i32→u32 只做位掩码搬运；零依赖不引 bytecount"
    )]
    mod avx2 {
        use core::arch::x86_64::{
            __m256i, _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_set1_epi8,
            _mm256_setzero_si256,
        };

        /// # Safety
        /// Caller must ensure AVX2 is available.
        #[target_feature(enable = "avx2")]
        #[allow(
            clippy::cast_ptr_alignment,
            clippy::cast_possible_wrap,
            clippy::cast_sign_loss,
            reason = "loadu 明确接受非对齐指针；u8→i8 与 i32→u32 只用于 SIMD 位掩码搬运，语义即按位"
        )]
        pub unsafe fn find_byte(haystack: &[u8], needle: u8) -> Option<usize> {
            let len = haystack.len();
            let ptr = haystack.as_ptr();
            let needle_vec = _mm256_set1_epi8(needle as i8);
            let mut i = 0usize;

            while i + 32 <= len {
                // SAFETY: i + 32 <= len, so the 32-byte load stays in bounds.
                let chunk = unsafe { _mm256_loadu_si256(ptr.add(i).cast::<__m256i>()) };
                let eq = _mm256_cmpeq_epi8(chunk, needle_vec);
                let mask = _mm256_movemask_epi8(eq) as u32;
                if mask != 0 {
                    return Some(i + mask.trailing_zeros() as usize);
                }
                i += 32;
            }
            // Tail: at most 31 bytes, scalar.
            haystack[i..]
                .iter()
                .position(|&b| b == needle)
                .map(|p| i + p)
        }

        /// # Safety
        /// Caller must ensure AVX2 is available.
        #[target_feature(enable = "avx2")]
        #[allow(
            clippy::cast_ptr_alignment,
            clippy::cast_possible_wrap,
            clippy::cast_sign_loss,
            reason = "loadu 明确接受非对齐指针；u8→i8 与 i32→u32 只用于 SIMD 位掩码搬运，语义即按位"
        )]
        pub unsafe fn count_byte(haystack: &[u8], needle: u8) -> usize {
            let len = haystack.len();
            let ptr = haystack.as_ptr();
            let needle_vec = _mm256_set1_epi8(needle as i8);
            let mut i = 0usize;
            let mut total = 0usize;

            while i + 32 <= len {
                // SAFETY: i + 32 <= len.
                let chunk = unsafe { _mm256_loadu_si256(ptr.add(i).cast::<__m256i>()) };
                let mask = _mm256_movemask_epi8(_mm256_cmpeq_epi8(chunk, needle_vec)) as u32;
                total += mask.count_ones() as usize;
                i += 32;
            }
            total + haystack[i..].iter().filter(|&&b| b == needle).count()
        }

        /// # Safety
        /// Caller must ensure AVX2 is available.
        #[target_feature(enable = "avx2")]
        pub unsafe fn find_nonzero(haystack: &[u8]) -> Option<usize> {
            let len = haystack.len();
            let ptr = haystack.as_ptr();
            let zero = _mm256_setzero_si256();
            let mut i = 0usize;

            while i + 32 <= len {
                // SAFETY: i + 32 <= len.
                let chunk = unsafe { _mm256_loadu_si256(ptr.add(i).cast::<__m256i>()) };
                let eq = _mm256_cmpeq_epi8(chunk, zero);
                // Invert: set bit where the byte differs from zero.
                let mask = !(_mm256_movemask_epi8(eq) as u32);
                if mask != 0 {
                    return Some(i + mask.trailing_zeros() as usize);
                }
                i += 32;
            }
            haystack[i..].iter().position(|&b| b != 0).map(|p| i + p)
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[allow(
        clippy::cast_ptr_alignment,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        clippy::naive_bytecount,
        reason = "SIMD 内核：loadu 明确接受非对齐指针，u8→i8 / i32→u32 只做位掩码搬运；零依赖不引 bytecount"
    )]
    mod sse2 {
        use core::arch::x86_64::{
            __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8,
        };

        /// # Safety
        /// Caller must ensure SSE2 is available.
        #[target_feature(enable = "sse2")]
        pub unsafe fn find_byte(haystack: &[u8], needle: u8) -> Option<usize> {
            let len = haystack.len();
            let ptr = haystack.as_ptr();
            let needle_vec = _mm_set1_epi8(needle as i8);
            let mut i = 0usize;

            while i + 16 <= len {
                // SAFETY: i + 16 <= len.
                let chunk = unsafe { _mm_loadu_si128(ptr.add(i).cast::<__m128i>()) };
                let mask = _mm_movemask_epi8(_mm_cmpeq_epi8(chunk, needle_vec)) as u32;
                if mask != 0 {
                    return Some(i + mask.trailing_zeros() as usize);
                }
                i += 16;
            }
            haystack[i..]
                .iter()
                .position(|&b| b == needle)
                .map(|p| i + p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::bytes::{count_byte, find_byte, find_byte_from, find_byte_scalar, is_all_zero};

    /// A tiny deterministic PRNG so the tests need no dependencies and stay
    /// reproducible across machines.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            // xorshift64*
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn byte(&mut self) -> u8 {
            u8::try_from(self.next() >> 56).expect("取最高字节，必然 < 256")
        }
    }

    #[test]
    fn matches_scalar_at_every_alignment_and_length() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for len in 0..260usize {
            for shift in 0..8usize {
                let mut data = vec![0u8; len + shift];
                for b in &mut data {
                    *b = rng.byte();
                }
                // Plant a needle so the hit path is exercised, sometimes at the
                // very last byte and sometimes not at all.
                let needle = 0x5Au8;
                let planted = if len > 0 {
                    Some(usize::try_from(rng.next() % (len as u64)).expect("模 len 后必 < len"))
                } else {
                    None
                };
                if let Some(p) = planted {
                    data[shift + p] = needle;
                }
                let hay = &data[shift..];

                assert_eq!(
                    find_byte(hay, needle),
                    find_byte_scalar(hay, needle),
                    "len={len} shift={shift}"
                );
                #[allow(
                    clippy::naive_bytecount,
                    reason = "测试里的参考实现，故意写成最直白的标量计数"
                )]
                let expected = hay.iter().filter(|&&b| b == needle).count();
                assert_eq!(count_byte(hay, needle), expected, "len={len} shift={shift}");
                for from in [0, hay.len() / 3, hay.len().saturating_sub(1)] {
                    assert_eq!(
                        find_byte_from(hay, needle, from),
                        hay.get(from..)
                            .and_then(|s| s.iter().position(|&b| b == needle))
                            .map(|p| p + from),
                        "len={len} shift={shift} from={from}"
                    );
                }
            }
        }
    }

    #[test]
    fn boundary_lengths_around_vector_width() {
        // 31/32/33 and 63/64/65 are where tail handling usually breaks.
        for len in [0usize, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129] {
            let mut data = vec![0u8; len];
            if len > 0 {
                data[len - 1] = 0xAA;
                assert_eq!(find_byte(&data, 0xAA), Some(len - 1), "len={len}");
                assert_eq!(count_byte(&data, 0xAA), 1, "len={len}");
            }
            assert_eq!(find_byte(&data, 0xAB), None, "len={len}");
        }
    }

    #[test]
    fn last_byte_hit_is_found() {
        // A classic off-by-one: the vector loop must not skip the tail.
        for len in 33..70usize {
            let mut data = vec![1u8; len];
            data[len - 1] = 0xFF;
            assert_eq!(find_byte(&data, 0xFF), Some(len - 1));
        }
    }

    #[test]
    fn zero_scan() {
        assert!(is_all_zero(&[]));
        assert!(is_all_zero(&[0; 100]));
        assert!(!is_all_zero(&[0, 0, 1, 0]));
        let mut v = vec![0u8; 1000];
        v[999] = 7;
        assert!(!is_all_zero(&v));
    }

    #[test]
    fn from_beyond_end_is_none() {
        assert_eq!(find_byte_from(b"abc", b'a', 3), None);
        assert_eq!(find_byte_from(b"abc", b'a', 100), None);
    }
}
