//! 三值 Quine-McCluskey（QM）化简：真值表 → 最简积之和（SOP，sum-of-products）。
//!
//! # 概念
//!
//! 一个**积项**是 `Vec<Ternary>`：某一位上是 [`Ternary::Unknown`] 表示 wildcard
//! （通配，匹配 `False / Unknown / True` 三个值中的任意一个）；`False` / `True`
//! 表示该位必须严格等于这个值。一组积项之间是「逻辑或」：只要有任意一条积项
//! 匹配某个具体赋值，函数就在该赋值上输出 `True`。
//!
//! 输入 `rows: &[(Vec<Ternary>, Ternary)]` 是真值表：每行是一个具体赋值 + 该赋值的
//! 输出。我们只把输出为 `True` 的赋值当作「最小项」（minterm），做 QM 合并，最后选
//! 一组质蕴含项（prime implicant）覆盖全部 True 最小项。
//!
//! # QM 流程（[`minimize`]）
//!
//! 1. **校验**：行数、每个输入向量的长度必须一致；同一输入向量重复出现但输出矛盾时
//!    返回 [`CodecError::TruthTableConflict`]。输出为 `False` / `Unknown` 的行不参与
//!    True 覆盖，但参与冲突检测。
//! 2. **收集 True 集合**：把所有输出 `True` 的具体赋值收进 `true_set`。
//! 3. **迭代合并**：反复两两配对。两条积项**仅在一位**上一个是 `False`、另一个是
//!    `True` 时才能合并，合并后该位变 `Unknown`；其余位必须全等（**含 wildcard
//!    位置也要完全一致**）。合并成功即把双方标记为「已吸收」。
//! 4. **不过度覆盖护栏**：与二进制 QM 不同，三值 wildcard 一次匹配三个值。合并出的
//!    wildcard 项可能引入从未声明为 True 的赋值。因此每次合并后都枚举 `3^n` 个赋值
//!    验证：该项覆盖到的每一个赋值都必须落在 `true_set` 里。否则拒绝这次合并，宁可多留
//!    一条最小项。
//! 5. **提取质蕴含项**：本轮没被任何有效合并吸收的项即为质蕴含项。
//! 6. **选最小覆盖**：先选「本质质蕴含项」（唯一覆盖某个最小项的质蕴含项），再贪心选
//!    覆盖剩余未覆盖最小项最多的项。
//!
//! # 已知局限
//!
//! - **最简形式不唯一**：贪心集合覆盖只产出 *一个* 较小的覆盖，不保证项数达到全局
//!   最小，也不保证逐字等于某个规范形式。正确性由「覆盖集合逐点等价」钉死（见
//!   等价性测试），而不是由项数钉死。
//! - **复杂度**：`term_is_valid_implicant` 每次合并都枚举 `3^n` 个赋值，仅供小 `n`
//!   （实验层）使用；`n` 很大时不要调用。

use crate::CodecError;
use trit_core::value::Ternary;

/// 一条积项：`Unknown` = wildcard（匹配任意三值），`False`/`True` = 必须匹配。
pub type Term = Vec<Ternary>;

/// 判断两个积项能否合并：仅一位互补（`False`/`True`），其余位全等（含 wildcard 位置）。
///
/// 返回合并后的积项（互补位置 `Unknown`）；形状不满足（多于一位互补、或某一位是
/// wildcard 与具体值相邻）时返回 `None`。
fn try_merge_shape(a: &[Ternary], b: &[Ternary]) -> Option<Term> {
    if a.len() != b.len() {
        return None;
    }
    let mut diff_pos: Option<usize> = None;
    for i in 0..a.len() {
        if a[i] == b[i] {
            continue;
        }
        match (a[i], b[i]) {
            (Ternary::False, Ternary::True) | (Ternary::True, Ternary::False) => {
                if diff_pos.is_some() {
                    return None;
                }
                diff_pos = Some(i);
            }
            // 其余任何不一致（含 wildcard 与具体值相邻）都不可合并。
            _ => return None,
        }
    }
    match diff_pos {
        None => None,
        Some(i) => {
            let mut merged = a.to_vec();
            merged[i] = Ternary::Unknown;
            Some(merged)
        }
    }
}

/// 一个积项是否覆盖某个具体赋值：逐位要求 `term[i]` 是 wildcard，或严格等于 `assign[i]`。
pub fn term_covers(term: &[Ternary], assign: &[Ternary]) -> bool {
    term.len() == assign.len()
        && term
            .iter()
            .zip(assign.iter())
            .all(|(t, a)| t.is_unknown() || t == a)
}

/// 枚举 `3^n` 个赋值，返回第 `mask` 个（LSB 在前）。
fn nth_assignment(mask: usize, n: usize) -> Vec<Ternary> {
    let mut out = Vec::with_capacity(n);
    let mut rem = mask;
    for _ in 0..n {
        out.push(Ternary::from_index(rem % 3));
        rem /= 3;
    }
    out
}

/// 校验：`term` 覆盖到的每一个具体赋值都落在 `true_set` 里（不过度覆盖非 True 行）。
fn term_is_valid_implicant(
    term: &[Ternary],
    true_set: &std::collections::HashSet<Vec<Ternary>>,
    n: usize,
) -> bool {
    (0..3usize.pow(n as u32))
        .map(|mask| nth_assignment(mask, n))
        .all(|assign| !term_covers(term, &assign) || true_set.contains(&assign))
}

/// 真值表化简：返回一组积项，其覆盖集合与 `rows` 中输出 `True` 的行集合**逐点相等**。
///
/// 返回的项不保证项数最少（见模块级文档的「已知局限」），但保证既不漏掉任何 True 行、
/// 也不误覆盖任何非 True 行。
///
/// # 错误
///
/// - 输入向量长度不一致、或同一输入向量重复出现但输出矛盾 →
///   [`CodecError::TruthTableConflict`]。
pub fn minimize(rows: &[(Vec<Ternary>, Ternary)]) -> Result<Vec<Term>, CodecError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let n = rows[0].0.len();

    // 冲突检测 + 收集 True 集合。
    let mut seen: std::collections::HashMap<Vec<Ternary>, Ternary> =
        std::collections::HashMap::new();
    let mut true_set: std::collections::HashSet<Vec<Ternary>> = std::collections::HashSet::new();
    for (row_idx, (input, out)) in rows.iter().enumerate() {
        if input.len() != n {
            return Err(CodecError::TruthTableConflict { row: row_idx });
        }
        match seen.get(input) {
            Some(prev) if *prev != *out => {
                return Err(CodecError::TruthTableConflict { row: row_idx });
            }
            None => {
                seen.insert(input.clone(), *out);
            }
            _ => {}
        }
        if out.is_true() {
            true_set.insert(input.clone());
        }
    }

    if true_set.is_empty() {
        return Ok(Vec::new());
    }

    // QM 反复合并，只接受不过度覆盖的合并结果。
    let mut current: Vec<Term> = true_set.iter().cloned().collect();
    let mut primes: Vec<Term> = Vec::new();
    while !current.is_empty() {
        let mut used = vec![false; current.len()];
        let mut next: Vec<Term> = Vec::new();

        for i in 0..current.len() {
            for j in (i + 1)..current.len() {
                let Some(merged) = try_merge_shape(&current[i], &current[j]) else {
                    continue;
                };
                // 形状可合并后，先验证不过度覆盖；一旦接受，两位都视为「已吸收」，
                // 即便 merged 与 next 里已有的项重复（去重只影响是否再次入队）。
                if !term_is_valid_implicant(&merged, &true_set, n) {
                    continue;
                }
                used[i] = true;
                used[j] = true;
                if !next.contains(&merged) {
                    next.push(merged);
                }
            }
        }

        // 本轮没被任何有效合并吸收的项 → 质蕴含项。
        for (term, was_used) in current.iter().zip(used.iter()) {
            if !was_used && !primes.contains(term) {
                primes.push(term.clone());
            }
        }

        current = next;
    }

    // 质蕴含项覆盖表：covered_by[p] = 第 p 个质蕴含项覆盖的最小项下标集合。
    let minterms: Vec<&Term> = true_set.iter().collect();
    let covered_by: Vec<Vec<usize>> = primes
        .iter()
        .map(|p| {
            minterms
                .iter()
                .enumerate()
                .filter(|(_, m)| term_covers(p, m))
                .map(|(i, _)| i)
                .collect()
        })
        .collect();

    // 选最小覆盖：本质项优先，剩下贪心。
    let mut uncovered: Vec<bool> = vec![true; minterms.len()];
    let mut chosen: Vec<usize> = Vec::new();
    loop {
        // 找第一个「只被唯一一个质蕴含项覆盖」的未覆盖最小项 → 本质项。
        let essential = uncovered
            .iter()
            .enumerate()
            .filter(|&(_, flag)| *flag)
            .find_map(|(m, _)| {
                let covering: Vec<usize> = covered_by
                    .iter()
                    .enumerate()
                    .filter(|(_, list)| list.contains(&m))
                    .map(|(pi, _)| pi)
                    .collect();
                (covering.len() == 1).then_some(covering[0])
            });

        if let Some(pi) = essential {
            chosen.push(pi);
            for &m in &covered_by[pi] {
                uncovered[m] = false;
            }
            continue;
        }

        // 无本质项：贪心选覆盖未覆盖最小项最多的未选质蕴含项（平局取先出现者）。
        let best = covered_by
            .iter()
            .enumerate()
            .filter(|(pi, _)| !chosen.contains(pi))
            .map(|(pi, list)| (pi, list.iter().filter(|&&m| uncovered[m]).count()))
            .fold(None, |acc: Option<(usize, usize)>, (pi, c)| match acc {
                None if c > 0 => Some((pi, c)),
                Some((_, best_count)) if c > best_count => Some((pi, c)),
                _ => acc,
            })
            .map(|(pi, _)| pi);

        match best {
            None => break,
            Some(pi) => {
                chosen.push(pi);
                for &m in &covered_by[pi] {
                    uncovered[m] = false;
                }
            }
        }
    }

    let result: Vec<Term> = chosen.into_iter().map(|pi| primes[pi].clone()).collect();

    // 最后一道护栏：枚举 3^n 个赋值，确认选出的积项覆盖集合与 true_set **逐点相等**。
    // 合并阶段的 guard 只约束「合并产物」，原始最小项（尤其含 Unknown 的行）若落在非
    // 盒状的真值表上会过度覆盖；这里统一兜住，把静默过度覆盖变成结构化 Err。
    for mask in 0..3usize.pow(n as u32) {
        let assign = nth_assignment(mask, n);
        let covered = result.iter().any(|term| term_covers(term, &assign));
        if covered != true_set.contains(&assign) {
            return Err(CodecError::TruthTableNotRepresentable);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Vec<Ternary> {
        s.chars().map(|c| Ternary::parse_char(c).unwrap()).collect()
    }

    /// 枚举 n 位全部 3^n 赋值，看 terms 覆盖的集合是否 == expected_true 集合。
    fn coverage_equals(terms: &[Term], n: usize, expected_true: &[Vec<Ternary>]) -> bool {
        let mut covered: std::collections::BTreeSet<Vec<Ternary>> =
            std::collections::BTreeSet::new();
        for mask in 0..3usize.pow(n as u32) {
            let assign = nth_assignment(mask, n);
            if terms.iter().any(|term| term_covers(term, &assign)) {
                covered.insert(assign);
            }
        }
        let expect: std::collections::BTreeSet<Vec<Ternary>> =
            expected_true.iter().cloned().collect();
        covered == expect
    }

    /// 枚举 n=3 全部 27 行，按 pred 决定输出 True/False。
    fn full_table_3(
        pred: impl Fn(Ternary, Ternary, Ternary) -> bool,
    ) -> Vec<(Vec<Ternary>, Ternary)> {
        let mut rows = Vec::new();
        for a in Ternary::ALL {
            for b in Ternary::ALL {
                for c in Ternary::ALL {
                    let input = vec![a, b, c];
                    let out = if pred(a, b, c) {
                        Ternary::True
                    } else {
                        Ternary::False
                    };
                    rows.push((input, out));
                }
            }
        }
        rows
    }

    /// 确定性 LCG（不引入新依赖），用于等价性随机测试。
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0
        }
    }

    #[test]
    fn empty_rows_give_empty_sop() {
        assert!(minimize(&[]).unwrap().is_empty());
    }

    #[test]
    fn single_true_row_is_its_own_minterm() {
        // f=1 仅在 (0,0)，其余 8 行 False。
        let rows = (0..9)
            .map(|mask| {
                let assign = nth_assignment(mask, 2);
                let out = if assign == t("00") {
                    Ternary::True
                } else {
                    Ternary::False
                };
                (assign, out)
            })
            .collect::<Vec<_>>();
        let sop = minimize(&rows).unwrap();
        assert!(coverage_equals(&sop, 2, &[t("00")]), "sop={sop:?}");
    }

    #[test]
    fn b_constant_true_merges_to_wildcard() {
        // f=1 当且仅当 b=False（a,c 任意）→ 期望 SOP = ["X0X"]。
        let rows = full_table_3(|_a, b, _c| b.is_false());
        let sop = minimize(&rows).unwrap();
        assert_eq!(sop, vec![t("X0X")]);
    }

    #[test]
    fn two_minterms_dont_overmerge_when_third_is_false() {
        // f=1 在 (0,0) 和 (1,0)，但 (X,0)=False。
        // 不能合并成 "X0"（会过度覆盖 (X,0)），应保留两条最小项。
        let rows = (0..9)
            .map(|mask| {
                let assign = nth_assignment(mask, 2);
                let is_true = assign == t("00") || assign == t("10");
                (
                    assign,
                    if is_true {
                        Ternary::True
                    } else {
                        Ternary::False
                    },
                )
            })
            .collect::<Vec<_>>();
        let sop = minimize(&rows).unwrap();
        // 不强制项数，只断言覆盖等价。
        assert!(coverage_equals(&sop, 2, &[t("00"), t("10")]), "sop={sop:?}");
        // 而且 sop 里不应该出现 "X0"（会过度覆盖）。
        assert!(!sop.iter().any(|term| term == &t("X0")), "sop={sop:?}");
    }

    #[test]
    fn truth_table_conflict_is_rejected() {
        let rows = vec![(t("00"), Ternary::True), (t("00"), Ternary::False)];
        assert!(matches!(
            minimize(&rows),
            Err(CodecError::TruthTableConflict { row: 1 })
        ));
    }

    #[test]
    fn duplicate_true_rows_are_deduped() {
        let rows = vec![
            (t("00"), Ternary::True),
            (t("00"), Ternary::True),
            (t("11"), Ternary::False),
        ];
        let sop = minimize(&rows).unwrap();
        assert!(coverage_equals(&sop, 2, &[t("00")]));
    }

    #[test]
    fn no_true_rows_give_empty_sop() {
        let rows = vec![(t("00"), Ternary::False), (t("11"), Ternary::False)];
        assert!(minimize(&rows).unwrap().is_empty());
    }

    #[test]
    fn equivalence_on_sparse_true_set() {
        // True 行：(0,0,0), (1,0,0), (1,1,1), (0,1,1)。
        // 这些行两两合并时，第三位取 Unknown 都不在 True 集合里 → 不合并，保留 4 条最小项。
        let true_rows = vec![t("000"), t("100"), t("111"), t("011")];
        let rows = full_table_3(|a, b, c| true_rows.contains(&vec![a, b, c]));
        let sop = minimize(&rows).unwrap();
        assert!(coverage_equals(&sop, 3, &true_rows), "sop={sop:?}");
    }

    #[test]
    fn equivalence_on_fully_constant_function() {
        // f ≡ True（所有 27 行）→ SOP = ["XXX"]。
        let rows = full_table_3(|_, _, _| true);
        let sop = minimize(&rows).unwrap();
        assert_eq!(sop, vec![t("XXX")]);
    }

    // ---- 新增：覆盖等价性强化测试 ----

    #[test]
    fn equivalence_n2_or_of_two_inputs() {
        // f = (a=True) OR (b=True)；True 行共 8 行，唯一 False 是 (F,F)。
        let rows = (0..9)
            .map(|mask| {
                let assign = nth_assignment(mask, 2);
                let out = if assign[0].is_true() || assign[1].is_true() {
                    Ternary::True
                } else {
                    Ternary::False
                };
                (assign, out)
            })
            .collect::<Vec<_>>();
        let true_rows = (0..9)
            .filter_map(|mask| {
                let assign = nth_assignment(mask, 2);
                (assign[0].is_true() || assign[1].is_true()).then_some(assign)
            })
            .collect::<Vec<_>>();
        let sop = minimize(&rows).unwrap();
        assert!(coverage_equals(&sop, 2, &true_rows), "sop={sop:?}");
    }

    #[test]
    fn equivalence_n3_b_true_or_c_unknown() {
        // f = (b=False) OR (c=True)。混合 wildcard，断言逐点等价即可。
        let rows = full_table_3(|_a, b, c| b.is_false() || c.is_true());
        let true_rows = (0..27)
            .filter_map(|mask| {
                let assign = nth_assignment(mask, 3);
                (assign[1].is_false() || assign[2].is_true()).then_some(assign)
            })
            .collect::<Vec<_>>();
        let sop = minimize(&rows).unwrap();
        assert!(coverage_equals(&sop, 3, &true_rows), "sop={sop:?}");
    }

    /// 展开结果：(完整真值表 rows, 其中 True 行的具体赋值)。
    type Table = (Vec<(Vec<Ternary>, Ternary)>, Vec<Vec<Ternary>>);

    /// 由一组随机积项（盒子）展开成真值表：盒状并集天然是 SOP 可表示的。
    fn rows_from_random_boxes(boxes: &[Term], n: usize) -> Table {
        let mut rows = Vec::new();
        let mut true_rows = Vec::new();
        for mask in 0..3usize.pow(n as u32) {
            let assign = nth_assignment(mask, n);
            let is_true = boxes.iter().any(|b| term_covers(b, &assign));
            let out = if is_true {
                Ternary::True
            } else {
                Ternary::False
            };
            if is_true {
                true_rows.push(assign.clone());
            }
            rows.push((assign, out));
        }
        (rows, true_rows)
    }

    /// 随机生成一条积项：每位按 0.35 False / 0.35 True / 0.30 Unknown 取值。
    fn random_box(rng: &mut Rng, n: usize) -> Term {
        (0..n)
            .map(|_| match rng.next() % 10 {
                0..=3 => Ternary::False,
                4..=7 => Ternary::True,
                _ => Ternary::Unknown,
            })
            .collect()
    }

    #[test]
    fn randomized_equivalence_n2() {
        // 随机生成盒状真值表，断言化简结果覆盖集合 == 展开出的 True 行集合。
        let mut rng = Rng(0xA5A5_C0DE_1234_5678);
        for _ in 0..300 {
            let k = 1 + (rng.next() % 3) as usize;
            let boxes: Vec<Term> = (0..k).map(|_| random_box(&mut rng, 2)).collect();
            let (rows, true_rows) = rows_from_random_boxes(&boxes, 2);
            let sop = minimize(&rows).expect("盒状真值表必须可表示");
            assert!(
                coverage_equals(&sop, 2, &true_rows),
                "boxes={boxes:?}\nsop={sop:?}"
            );
        }
    }

    #[test]
    fn randomized_equivalence_n3() {
        let mut rng = Rng(0xBEEF_0BAD_CAFE_D00D);
        for _ in 0..150 {
            let k = 1 + (rng.next() % 3) as usize;
            let boxes: Vec<Term> = (0..k).map(|_| random_box(&mut rng, 3)).collect();
            let (rows, true_rows) = rows_from_random_boxes(&boxes, 3);
            let sop = minimize(&rows).expect("盒状真值表必须可表示");
            assert!(
                coverage_equals(&sop, 3, &true_rows),
                "boxes={boxes:?}\nsop={sop:?}"
            );
        }
    }

    #[test]
    fn non_representable_table_is_rejected_not_overcovered() {
        // True 行 = {(U,U),(U,F),(U,T),(F,U)}：UF 无法用任何不过度覆盖的盒子覆盖
        // （覆盖 (U,F) 必然 wildcards 出 FF/TF，而它们是 False）。必须 Err，而非过度覆盖。
        let rows = vec![
            (t("00"), Ternary::False),
            (t("X0"), Ternary::True),
            (t("10"), Ternary::False),
            (t("0X"), Ternary::True),
            (t("XX"), Ternary::True),
            (t("1X"), Ternary::False),
            (t("01"), Ternary::False),
            (t("X1"), Ternary::True),
            (t("11"), Ternary::False),
        ];
        assert!(matches!(
            minimize(&rows),
            Err(CodecError::TruthTableNotRepresentable)
        ));
    }
}
