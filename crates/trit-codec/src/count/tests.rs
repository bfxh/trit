//! count 的单元测试（从 src/count.rs 拆出，控制单文件行数预算）。

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
        Err(CodecError::TermSetTooLarge { found: 64, max: 20 })
    ));
}

#[test]
fn cube_size_bounds_and_huge_n_does_not_wrap() {
    // M2 补充：`n as u32` 截断会让 n=2^32→0→3^0=1 绕过 CubeTooLarge；cube_size 用 u32::try_from 拒绝。
    assert_eq!(cube_size(0), Some(1));
    assert_eq!(cube_size(MAX_N), Some(3u128.pow(MAX_N as u32)));
    assert_eq!(cube_size(MAX_N + 1), None, "3^41 放不进 u64");
    assert_eq!(
        cube_size(1usize << 32),
        None,
        "超出 u32 的位数不得被截断成小指数"
    );
    assert_eq!(cube_size(u32::MAX as usize + 1), None);
}
