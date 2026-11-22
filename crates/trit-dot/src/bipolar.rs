//! 二值（`±1`）点积（#590）。
//!
//! 长度跟着 [`PackedBipolar`] 走；两个向量的 `len` 不同才返回
//! [`DotError::LengthMismatch`]，不再接受调用方单独传的 `len`。

use super::DotError;
use super::packed::PackedBipolar;
#[cfg(target_arch = "x86_64")]
use super::popcnt;

fn check_pair(a: &PackedBipolar, b: &PackedBipolar) -> Result<(), DotError> {
    if a.len() != b.len() {
        return Err(DotError::LengthMismatch {
            left: a.len(),
            right: b.len(),
        });
    }
    Ok(())
}

/// 二值（`±1`）点积：`n - 2·popcount(a XOR b)`，也就是「XNOR + POPC」。
///
/// 与三值路径一样走硬件 `popcnt` 派发——对照必须同待遇，不然比的不是编码而是
/// 谁碰巧拿到了指令。
pub fn bipolar_dot(a: &PackedBipolar, b: &PackedBipolar) -> Result<i64, DotError> {
    check_pair(a, b)?;
    #[cfg(target_arch = "x86_64")]
    if let Some(mismatches) = popcnt::bipolar_mismatches_or_zero(a.words(), b.words()) {
        return Ok(a.len() as i64 - 2 * mismatches as i64);
    }
    bipolar_dot_portable(a, b)
}

/// 便携版二值点积（不做派发），用来量派发本身值多少。
pub fn bipolar_dot_portable(a: &PackedBipolar, b: &PackedBipolar) -> Result<i64, DotError> {
    check_pair(a, b)?;
    let mismatches: u64 = a
        .words()
        .iter()
        .zip(b.words().iter())
        .map(|(left, right)| u64::from((left ^ right).count_ones()))
        .sum();
    Ok(a.len() as i64 - 2 * mismatches as i64)
}
