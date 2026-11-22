//! 二值（`±1`）点积与位打包（#590）。
//!
//! 长度契约：两个平面等长，且打包字数覆盖 `len` 个比特；不满足返回
//! [`DotError::LengthMismatch`]，不再按较短平面静默截断。

#[cfg(target_arch = "x86_64")]
use super::popcnt;
use super::{BLOCK_BITS, DotError};

/// 二值（`±1`）向量的位打包：`1 = +1`、`0 = -1`，**尾部补 1**。
pub fn pack_bipolar(values: &[i32]) -> Vec<u64> {
    let blocks = values.len().div_ceil(BLOCK_BITS);
    let mut words = vec![u64::MAX; blocks];
    for (index, value) in values.iter().enumerate() {
        if *value < 0 {
            words[index / BLOCK_BITS] &= !(1u64 << (index % BLOCK_BITS));
        }
    }
    words
}

/// 二值点积的长度契约：两个平面等长，且打包字数覆盖 `len` 个比特。
///
/// 旧实现直接 `zip`（按较短平面静默截断）再套 `len - 2·mismatches`，
/// 谎报 `len` 会得到"看起来合理"的错值而不是错误（#590）。
fn check_bipolar(a: &[u64], b: &[u64], len: usize) -> Result<(), DotError> {
    if a.len() != b.len() {
        return Err(DotError::LengthMismatch {
            left: a.len(),
            right: b.len(),
        });
    }
    let covered = a
        .len()
        .checked_mul(BLOCK_BITS)
        .ok_or_else(|| DotError::Shape("打包字数溢出 usize".to_string()))?;
    if covered < len {
        return Err(DotError::LengthMismatch {
            left: covered,
            right: len,
        });
    }
    Ok(())
}

/// 二值（`±1`）点积：`n - 2·popcount(a XOR b)`，也就是「XNOR + POPC」。
///
/// 与三值路径一样走硬件 `popcnt` 派发——对照必须同待遇，不然比的不是编码而是
/// 谁碰巧拿到了指令。
///
/// 长度不满足 [`check_bipolar`] 的契约时返回 [`DotError::LengthMismatch`]，
/// 不再按较短平面静默截断（#590）。
pub fn bipolar_dot(a: &[u64], b: &[u64], len: usize) -> Result<i64, DotError> {
    check_bipolar(a, b, len)?;
    #[cfg(target_arch = "x86_64")]
    if let Some(mismatches) = popcnt::bipolar_mismatches_or_zero(a, b) {
        return Ok(len as i64 - 2 * mismatches as i64);
    }
    bipolar_dot_portable(a, b, len)
}

/// 便携版二值点积（不做派发），用来量派发本身值多少。
pub fn bipolar_dot_portable(a: &[u64], b: &[u64], len: usize) -> Result<i64, DotError> {
    check_bipolar(a, b, len)?;
    let mismatches: u64 = a
        .iter()
        .zip(b.iter())
        .map(|(left, right)| u64::from((left ^ right).count_ones()))
        .sum();
    Ok(len as i64 - 2 * mismatches as i64)
}
