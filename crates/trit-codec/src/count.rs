//! 三值 #SAT：数一组 SOP 积项覆盖了 `3^n` 立方体中多少个具体赋值。
//!
//! 一条积项有 `w` 个 wildcard（`Unknown`）位，就覆盖 `3^w` 个具体赋值；多条积项
//! 求并集（重叠部分不能重复数）。这里用**容斥原理**精确计算：
//!
//! ```text
//! |∪ T_i| = Σ|T_i| - Σ|T_i ∩ T_j| + Σ|T_i ∩ T_j ∩ T_k| - ...
//! ```
//!
//! 两条积项相交：逐位合并——两位都 wildcard 仍是 wildcard；一位 wildcard 另一位
//! 具体则取具体；两位都是具体值且相等则保留；两位都是具体值但不同 → 交集为空。
//!
//! 边界：
//!
//! - 空 terms → 0；
//! - 单条全 wildcard 项（n 位）→ `3^n`；
//! - 所有积项长度必须一致，否则 [`CodecError::TermArityMismatch`]；
//! - 位数 `n > 40` 时 `3^n` 超出 `u64` 返回范围（`3^40 < u64::MAX < 3^41`），
//!   返回 [`CodecError::CubeTooLarge`]，绝不静默截断；
//! - 积项数 `m > 63` 时 `2^m` 连 `u64` bitmask 都表示不了，返回
//!   [`CodecError::TermSetTooLarge`]，绝不 panic。实用上限 `m ≤ 20`
//!   （2^20 ≈ 1M 个子集）；m 再大请换 BDD / ZDD，本模块不做。

use crate::CodecError;
use trit_core::value::Ternary;

/// 积项数硬上限：容斥要枚举 `2^m` 个非空子集，bitmask 用 `u64`，m ≥ 64 必溢出。
const MAX_TERMS: usize = 20;

/// 位数硬上限：返回类型是 `u64`，而并集大小 ≤ `3^n`；`3^40 < u64::MAX < 3^41`。
const MAX_N: usize = 40;

/// 两条积项求交：返回 `Some(intersection)`，冲突（两位都是具体值且不等）返回 `None`。
fn intersect(a: &[Ternary], b: &[Ternary]) -> Option<Vec<Ternary>> {
    if a.len() != b.len() {
        return None;
    }
    let mut out = Vec::with_capacity(a.len());
    for i in 0..a.len() {
        let c = match (a[i], b[i]) {
            (x, y) if x == y => x,
            (x, Ternary::Unknown) => x,
            (Ternary::Unknown, y) => y,
            _ => return None, // 两个不同的具体值
        };
        out.push(c);
    }
    Some(out)
}

/// 立方体 `3^n`（u128）：`n` 超出 `u32`（`n as u32` 会被截断、比如 2^32→0）
/// 或结果放不进 `u64` 时返回 `None`，交调用方报 `CubeTooLarge`——绝不静默截断（M2 补充）。
fn cube_size(n: usize) -> Option<u128> {
    let exp = u32::try_from(n).ok()?;
    let cube = 3u128.checked_pow(exp)?;
    (cube <= u64::MAX as u128).then_some(cube)
}

/// 数一组积项覆盖的具体赋值总数（并集大小，重叠部分只计一次）。
///
/// 返回 `|T_1 ∪ ... ∪ T_m|`：在全部 `3^n` 个具体赋值里，至少被一条积项覆盖的
/// 赋值个数。用容斥原理精确计数（不是 DFS 采样，不重不漏）。
///
/// # 边界与错误
///
/// - `terms` 为空 → `Ok(0)`；
/// - 所有积项长度必须一致，否则 [`CodecError::TermArityMismatch`]；
/// - `3^n` 超出 `u64` 时返回 [`CodecError::CubeTooLarge`]（n > `MAX_N`），
///   绝不静默截断或返回 0；
/// - 积项数 m > `MAX_TERMS` 时返回 [`CodecError::TermSetTooLarge`]，
///   绝不 panic。
pub fn count(terms: &[Vec<Ternary>]) -> Result<u64, CodecError> {
    if terms.is_empty() {
        return Ok(0);
    }
    let n = terms[0].len();
    for t in terms {
        if t.len() != n {
            return Err(CodecError::TermArityMismatch {
                expected: n,
                found: t.len(),
            });
        }
    }

    let m = terms.len();
    if m > MAX_TERMS {
        return Err(CodecError::TermSetTooLarge {
            found: m,
            max: MAX_TERMS,
        });
    }
    // 并集大小 ≤ 3^n；先确认 3^n 能放进 u64，后面所有子集规模都不可能更大。
    let cube = cube_size(n).ok_or(CodecError::CubeTooLarge { n, max: MAX_N })?;
    debug_assert!(cube > 0);

    let mut total: i128 = 0;
    // 枚举所有非空子集（bitmask）。
    for mask in 1..(1u64 << m) {
        let bits = mask.count_ones();
        // 交集初始 = 全 wildcard（n 个 Unknown）。
        let mut inter: Option<Vec<Ternary>> = Some(vec![Ternary::Unknown; n]);
        for (i, term) in terms.iter().enumerate() {
            if mask & (1u64 << i) == 0 {
                continue;
            }
            inter = match inter {
                Some(cur) => intersect(&cur, term),
                None => None,
            };
            if inter.is_none() {
                break;
            }
        }
        if let Some(inter) = inter {
            let w = inter.iter().filter(|t| t.is_unknown()).count();
            // w ≤ n ≤ 40，3^w 远小于 u128，直接幂（不再可能溢出后被误当 0）。
            let size = 3u128.pow(w as u32);
            if bits % 2 == 1 {
                total += size as i128;
            } else {
                total -= size as i128;
            }
        }
    }
    // 容斥结果必为 [0, 3^n] 的整数；守卫已保证该区间全在 u64 内。
    debug_assert!(total >= 0 && total <= cube as i128);
    Ok(total as u64)
}

#[cfg(test)]
mod tests;
