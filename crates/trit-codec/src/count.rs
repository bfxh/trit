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
const MAX_TERMS: usize = 63;

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
    let cube = 3u128
        .checked_pow(n as u32)
        .filter(|c| *c <= u64::MAX as u128)
        .ok_or(CodecError::CubeTooLarge { n, max: MAX_N })?;
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
mod tests {
    use super::*;

    fn t(s: &str) -> Vec<Ternary> {
        s.chars().map(|c| Ternary::parse_char(c).unwrap()).collect()
    }

    #[test]
    fn empty_terms_count_zero() {
        assert_eq!(count(&[]).unwrap(), 0);
    }

    #[test]
    fn single_concrete_term_covers_one_assignment() {
        // n=3，项 "01X"：w=1 → 3 个赋值。
        assert_eq!(count(&[t("01X")]).unwrap(), 3);
        // n=3，项 "010"：w=0 → 1 个。
        assert_eq!(count(&[t("010")]).unwrap(), 1);
    }

    #[test]
    fn single_all_wildcard_term_is_3_pow_n() {
        // n=3，全 wildcard → 27。
        assert_eq!(count(&[t("XXX")]).unwrap(), 27);
        // n=1 → 3。
        assert_eq!(count(&[t("X")]).unwrap(), 3);
    }

    #[test]
    fn hand_example_terms_0x1_and_100() {
        // 任务书手算：terms=["0X1","100"] 在 n=3 时并集 = 4。
        // "0X1" 覆盖 3 个（trit0=0, trit2=1, trit1 任意）。
        // "100" 覆盖 1 个（trit0=1, trit1=0, trit2=0）。
        // 交集：trit0 上 0 vs 1 冲突 → 空。并集 = 3+1 = 4。
        assert_eq!(count(&[t("0X1"), t("100")]).unwrap(), 4);
    }

    #[test]
    fn overlapping_terms_correctly_subtracted() {
        // "X0X"（b=0，覆盖 9 个）和 "X00"（b=0,c=0，覆盖 3 个）。
        // 并集 = 9 + 3 - 3 = 9（"X00" 完全被 "X0X" 包含）。
        assert_eq!(count(&[t("X0X"), t("X00")]).unwrap(), 9);
    }

    #[test]
    fn term_arity_mismatch_is_rejected() {
        let terms = vec![t("01"), t("X")];
        assert!(matches!(
            count(&terms),
            Err(CodecError::TermArityMismatch {
                expected: 2,
                found: 1
            })
        ));
    }

    #[test]
    fn cross_check_with_sop() {
        // 真值表：n=3，True 行 = {(0,0,0), (1,0,0), (1,1,1), (0,1,1)}。
        // 化简后 sop 的覆盖数应该 == 4（原 True 行数）。
        use crate::sop;
        let true_rows = [t("000"), t("100"), t("111"), t("011")];
        let mut rows = Vec::new();
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                for c in Ternary::ALL {
                    let input = vec![a, b, c];
                    let out = if true_rows.contains(&input) {
                        Ternary::True
                    } else {
                        Ternary::False
                    };
                    rows.push((input, out));
                }
            }
        }
        let terms = sop::minimize(&rows).unwrap();
        let covered = count(&terms).unwrap();
        assert_eq!(covered, true_rows.len() as u64);
    }

    #[test]
    fn cross_check_b_constant() {
        // f=1 iff b=0（9 行 True）。sop 应化简成 "X0X"，count 应 = 9。
        use crate::sop;
        let mut rows = Vec::new();
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                for c in Ternary::ALL {
                    let input = vec![a, b, c];
                    let out = if b.is_false() {
                        Ternary::True
                    } else {
                        Ternary::False
                    };
                    rows.push((input, out));
                }
            }
        }
        let terms = sop::minimize(&rows).unwrap();
        assert_eq!(count(&terms).unwrap(), 9);
    }

    #[test]
    fn cross_check_overlapping_primes() {
        // n=2，True 行 = 至少一位是 '0'(False)：{00,01,0X,10,X0}（共 5 行）。
        // QM 会合并出两条互相重叠的质蕴含项 "0X"（覆盖 3）和 "X0"（覆盖 3），
        // 交集 "00"（1）。容斥：3 + 3 - 1 = 5，必须 == 原 True 行数。
        use crate::sop;
        let true_rows = [t("00"), t("01"), t("0X"), t("10"), t("X0")];
        let mut rows = Vec::new();
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                let input = vec![a, b];
                let out = if true_rows.contains(&input) {
                    Ternary::True
                } else {
                    Ternary::False
                };
                rows.push((input, out));
            }
        }
        let terms = sop::minimize(&rows).unwrap();
        assert_eq!(count(&terms).unwrap(), true_rows.len() as u64);
    }

    #[test]
    fn cube_3_pow_40_is_represented_exactly() {
        // n=40 全 wildcard：3^40 < u64::MAX，必须精确返回，不能截断成 0。
        let term = t(&"X".repeat(40));
        assert_eq!(count(&[term]).unwrap(), 3u64.pow(40));
    }

    #[test]
    fn cube_3_pow_41_errors_instead_of_silent_truncation() {
        // n=41 时 3^41 > u64::MAX：旧实现 checked_pow 失败后静默返回 0，
        // 现在必须返回结构化错误。
        let term = t(&"X".repeat(41));
        assert!(matches!(
            count(&[term]),
            Err(CodecError::CubeTooLarge { n: 41, max: 40 })
        ));
    }

    #[test]
    fn too_many_terms_errors_instead_of_shift_panic() {
        // m=64：旧实现 `1u64 << 64` 直接 panic；现在必须返回结构化错误。
        let terms: Vec<Vec<Ternary>> = (0..64).map(|_| t("0")).collect();
        assert!(matches!(
            count(&terms),
            Err(CodecError::TermSetTooLarge { found: 64, max: 63 })
        ));
    }
}
