//! 位运算 SIMD 抽象：`popcount` 与 XOR-popcount（Hamming 距离）的**运行时派发**。
//!
//! 派发顺序：
//! - x86_64：AVX512（VPOPCNTDQ，需 rustc ≥ 1.89 编译 + CPU 支持）→ POPCNT → 标量；
//! - aarch64：NEON → 标量；
//! - 其它：标量。
//!
//! 口径：所有后端对同一输入给**同一个数**（后端只影响速度）；长度不一致返回结构化错误。
//! MSRV 1.88 上 AVX512 路径不会被编译（`build.rs` 按 rustc 版本发 cfg），见 crate 文档。

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(unsafe_code)]

use std::sync::OnceLock;

/// 实际使用的后端。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// AVX512 + VPOPCNTDQ。
    Avx512,
    /// x86 `POPCNT` 指令。
    Popcnt,
    /// aarch64 NEON。
    Neon,
    /// 纯标量。
    Scalar,
}

impl Backend {
    /// 稳定短名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Avx512 => "avx512",
            Self::Popcnt => "popcnt",
            Self::Neon => "neon",
            Self::Scalar => "scalar",
        }
    }
}

/// SIMD 错误。
#[derive(Debug, thiserror::Error)]
pub enum SimdError {
    /// 两条切片长度不一致。
    #[error("长度不一致：左 {left}，右 {right}")]
    LengthMismatch {
        /// 左侧长度。
        left: usize,
        /// 右侧长度。
        right: usize,
    },
}

static BACKEND: OnceLock<Backend> = OnceLock::new();

/// 当前进程选中的后端（只探测一次）。
#[must_use]
pub fn selected_backend() -> Backend {
    *BACKEND.get_or_init(detect)
}

#[allow(unreachable_code)]
fn detect() -> Backend {
    #[cfg(all(target_arch = "x86_64", has_avx512))]
    {
        if std::arch::is_x86_feature_detected!("avx512vpopcntdq") {
            return Backend::Avx512;
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("popcnt") {
            return Backend::Popcnt;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // NEON 是 aarch64 的必选扩展。
        return Backend::Neon;
    }
    Backend::Scalar
}

/// 数 `words` 里所有 1 的个数。
#[must_use]
pub fn popcount_words(words: &[u64]) -> u64 {
    popcount_words_with(selected_backend(), words)
}

/// 两条等长切片的 Hamming 距离（XOR 后数 1）。
pub fn hamming_words(left: &[u64], right: &[u64]) -> Result<u64, SimdError> {
    if left.len() != right.len() {
        return Err(SimdError::LengthMismatch {
            left: left.len(),
            right: right.len(),
        });
    }
    Ok(hamming_with(selected_backend(), left, right))
}

/// 指定后端数 1（不可用的后端自动退回标量）。
#[must_use]
pub fn popcount_words_with(backend: Backend, words: &[u64]) -> u64 {
    match backend {
        #[cfg(all(target_arch = "x86_64", has_avx512))]
        Backend::Avx512 => match avx512::popcount_or_zero(words) {
            Some(value) => value,
            None => scalar_popcount(words),
        },
        #[cfg(target_arch = "x86_64")]
        Backend::Popcnt => {
            popcnt::popcount_or_zero(words).unwrap_or_else(|| scalar_popcount(words))
        }
        #[cfg(target_arch = "aarch64")]
        Backend::Neon => neon::popcount(words),
        _ => scalar_popcount(words),
    }
}

/// 指定后端的 Hamming 距离（调用方保证等长）。
fn hamming_with(backend: Backend, left: &[u64], right: &[u64]) -> u64 {
    match backend {
        #[cfg(all(target_arch = "x86_64", has_avx512))]
        Backend::Avx512 => match avx512::hamming_or_zero(left, right) {
            Some(value) => value,
            None => scalar_hamming(left, right),
        },
        #[cfg(target_arch = "x86_64")]
        Backend::Popcnt => {
            popcnt::hamming_or_zero(left, right).unwrap_or_else(|| scalar_hamming(left, right))
        }
        #[cfg(target_arch = "aarch64")]
        Backend::Neon => neon::hamming(left, right),
        _ => scalar_hamming(left, right),
    }
}

fn scalar_popcount(words: &[u64]) -> u64 {
    words.iter().map(|word| u64::from(word.count_ones())).sum()
}

fn scalar_hamming(left: &[u64], right: &[u64]) -> u64 {
    left.iter()
        .zip(right)
        .map(|(a, b)| u64::from((a ^ b).count_ones()))
        .sum()
}

#[cfg(target_arch = "x86_64")]
mod popcnt {
    #[target_feature(enable = "popcnt")]
    unsafe fn popcount(words: &[u64]) -> u64 {
        words.iter().map(|word| u64::from(word.count_ones())).sum()
    }

    #[target_feature(enable = "popcnt")]
    unsafe fn hamming(left: &[u64], right: &[u64]) -> u64 {
        left.iter()
            .zip(right)
            .map(|(a, b)| u64::from((a ^ b).count_ones()))
            .sum()
    }

    pub(super) fn popcount_or_zero(words: &[u64]) -> Option<u64> {
        if !std::arch::is_x86_feature_detected!("popcnt") {
            return None;
        }
        // SAFETY: 上面的检测就是 POPCNT 指令的存在性。
        Some(unsafe { popcount(words) })
    }

    pub(super) fn hamming_or_zero(left: &[u64], right: &[u64]) -> Option<u64> {
        if !std::arch::is_x86_feature_detected!("popcnt") {
            return None;
        }
        // SAFETY: 同上。
        Some(unsafe { hamming(left, right) })
    }
}

#[cfg(all(target_arch = "x86_64", has_avx512))]
#[allow(clippy::incompatible_msrv)] // 模块本身按 rustc ≥ 1.89 的 cfg 才编译；1.88 构建里不存在
mod avx512 {
    use std::arch::x86_64::*;

    #[allow(unused_unsafe)] // 1.89+ 起 `_mm512_popcnt_epi64` 是安全函数，早期是 unsafe
    #[target_feature(enable = "avx512f,avx512vpopcntdq")]
    unsafe fn popcount(words: &[u64]) -> u64 {
        let mut total = 0u64;
        let mut index = 0;
        while index + 8 <= words.len() {
            // SAFETY: index + 8 ≤ len；未对齐加载允许。
            let values = unsafe { _mm512_loadu_si512(words.as_ptr().add(index) as *const __m512i) };
            let counted = unsafe { _mm512_popcnt_epi64(values) };
            let mut lanes = [0u64; 8];
            // SAFETY: lanes 正好 8 个 u64，宽度匹配 __m512i。
            unsafe { _mm512_storeu_si512(lanes.as_mut_ptr() as *mut __m512i, counted) };
            total += lanes.iter().sum::<u64>();
            index += 8;
        }
        for word in &words[index..] {
            total += u64::from(word.count_ones());
        }
        total
    }

    #[allow(unused_unsafe)]
    #[target_feature(enable = "avx512f,avx512vpopcntdq")]
    unsafe fn hamming(left: &[u64], right: &[u64]) -> u64 {
        let mut total = 0u64;
        let mut index = 0;
        while index + 8 <= left.len() {
            // SAFETY: index + 8 ≤ len。
            let a = unsafe { _mm512_loadu_si512(left.as_ptr().add(index) as *const __m512i) };
            let b = unsafe { _mm512_loadu_si512(right.as_ptr().add(index) as *const __m512i) };
            let xor = unsafe { _mm512_xor_si512(a, b) };
            let counted = unsafe { _mm512_popcnt_epi64(xor) };
            let mut lanes = [0u64; 8];
            unsafe { _mm512_storeu_si512(lanes.as_mut_ptr() as *mut __m512i, counted) };
            total += lanes.iter().sum::<u64>();
            index += 8;
        }
        for (&a, &b) in left[index..].iter().zip(&right[index..]) {
            total += u64::from((a ^ b).count_ones());
        }
        total
    }

    pub(super) fn popcount_or_zero(words: &[u64]) -> Option<u64> {
        if !std::arch::is_x86_feature_detected!("avx512vpopcntdq") {
            return None;
        }
        // SAFETY: 检测已确认 AVX512F + VPOPCNTDQ。
        Some(unsafe { popcount(words) })
    }

    pub(super) fn hamming_or_zero(left: &[u64], right: &[u64]) -> Option<u64> {
        if !std::arch::is_x86_feature_detected!("avx512vpopcntdq") {
            return None;
        }
        // SAFETY: 同上。
        Some(unsafe { hamming(left, right) })
    }
}

#[cfg(target_arch = "aarch64")]
mod neon {
    use std::arch::aarch64::*;

    #[allow(unused_unsafe)]
    #[target_feature(enable = "neon")]
    unsafe fn popcount_inner(words: &[u64]) -> u64 {
        let mut total = 0u64;
        let mut index = 0;
        while index + 2 <= words.len() {
            // SAFETY: index + 2 ≤ len，读取 16 字节。
            let block = unsafe { vld1q_u8(words.as_ptr().add(index) as *const u8) };
            let counted = vcntq_u8(block);
            total += u64::from(vaddlvq_u8(counted));
            index += 2;
        }
        for word in &words[index..] {
            total += u64::from(word.count_ones());
        }
        total
    }

    #[allow(unused_unsafe)]
    #[target_feature(enable = "neon")]
    unsafe fn hamming_inner(left: &[u64], right: &[u64]) -> u64 {
        let mut total = 0u64;
        let mut index = 0;
        while index + 2 <= left.len() {
            // SAFETY: index + 2 ≤ len，读取 16 字节。
            let a = unsafe { vld1q_u64(left.as_ptr().add(index)) };
            let b = unsafe { vld1q_u64(right.as_ptr().add(index)) };
            let xor = veorq_u64(a, b);
            let counted = vcntq_u8(vreinterpretq_u8_u64(xor));
            total += u64::from(vaddlvq_u8(counted));
            index += 2;
        }
        for (&a, &b) in left[index..].iter().zip(&right[index..]) {
            total += u64::from((a ^ b).count_ones());
        }
        total
    }

    pub(super) fn popcount(words: &[u64]) -> u64 {
        // SAFETY: aarch64 必选 NEON。
        unsafe { popcount_inner(words) }
    }

    pub(super) fn hamming(left: &[u64], right: &[u64]) -> u64 {
        // SAFETY: aarch64 必选 NEON。
        unsafe { hamming_inner(left, right) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
    }

    fn data(len: usize, seed: u64) -> Vec<u64> {
        let mut rng = Rng(seed);
        (0..len).map(|_| rng.next()).collect()
    }

    fn reference_popcount(words: &[u64]) -> u64 {
        words.iter().map(|word| u64::from(word.count_ones())).sum()
    }

    fn reference_hamming(left: &[u64], right: &[u64]) -> u64 {
        left.iter()
            .zip(right)
            .map(|(a, b)| u64::from((a ^ b).count_ones()))
            .sum()
    }

    #[test]
    fn all_lengths_match_scalar_reference() {
        for len in 0..=33 {
            let words = data(len, 7 + len as u64);
            let expected = reference_popcount(&words);
            assert_eq!(scalar_popcount(&words), expected, "scalar len={len}");
            assert_eq!(
                popcount_words_with(selected_backend(), &words),
                expected,
                "selected={:?} len={len}",
                selected_backend()
            );
        }
        let big = data(4096, 99);
        assert_eq!(popcount_words(&big), reference_popcount(&big));
    }

    #[test]
    fn forced_hardware_paths_match_scalar() {
        let words = data(4096 + 7, 11);
        let expected = reference_popcount(&words);
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("popcnt") {
                assert_eq!(popcount_words_with(Backend::Popcnt, &words), expected);
            }
        }
        #[cfg(all(target_arch = "x86_64", has_avx512))]
        {
            if std::arch::is_x86_feature_detected!("avx512vpopcntdq") {
                assert_eq!(popcount_words_with(Backend::Avx512, &words), expected);
            }
        }
    }

    #[test]
    fn hamming_matches_reference_and_is_symmetric() {
        for len in 0..=17 {
            let left = data(len, 21 + len as u64);
            let right = data(len, 100 + len as u64);
            let expected = reference_hamming(&left, &right);
            assert_eq!(hamming_words(&left, &right).unwrap(), expected, "len={len}");
            assert_eq!(hamming_words(&right, &left).unwrap(), expected, "len={len}");
            assert_eq!(scalar_hamming(&left, &right), expected, "len={len}");
        }
    }

    #[test]
    fn length_mismatch_is_an_error() {
        let err = hamming_words(&[1, 2], &[1]).unwrap_err();
        assert!(
            matches!(err, SimdError::LengthMismatch { left: 2, right: 1 }),
            "{err}"
        );
    }

    #[test]
    fn dispatch_uses_hardware_popcount_on_x86() {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("popcnt") {
                assert!(
                    matches!(selected_backend(), Backend::Popcnt | Backend::Avx512),
                    "selected={:?}",
                    selected_backend()
                );
            }
        }
    }

    #[test]
    fn results_are_deterministic() {
        let words = data(1024, 5);
        assert_eq!(popcount_words(&words), popcount_words(&words));
        let other = data(1024, 6);
        assert_eq!(
            hamming_words(&words, &other).unwrap(),
            hamming_words(&words, &other).unwrap()
        );
    }
}
