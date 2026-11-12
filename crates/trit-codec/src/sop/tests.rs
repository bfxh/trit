//! sop 的单元测试（从 src/sop.rs 拆出，控制单文件行数预算）。

use super::*;

fn t(s: &str) -> Vec<Ternary> {
    s.chars().map(|c| Ternary::parse_char(c).unwrap()).collect()
}

/// 枚举 n 位全部 3^n 赋值，看 terms 覆盖的集合是否 == expected_true 集合。
fn coverage_equals(terms: &[Term], n: usize, expected_true: &[Vec<Ternary>]) -> bool {
    let mut covered: std::collections::BTreeSet<Vec<Ternary>> = std::collections::BTreeSet::new();
    for mask in 0..3usize.pow(n as u32) {
        let assign = nth_assignment(mask, n);
        if terms.iter().any(|term| term_covers(term, &assign)) {
            covered.insert(assign);
        }
    }
    let expect: std::collections::BTreeSet<Vec<Ternary>> = expected_true.iter().cloned().collect();
    covered == expect
}

/// 枚举 n=3 全部 27 行，按 pred 决定输出 True/False。
fn full_table_3(pred: impl Fn(Ternary, Ternary, Ternary) -> bool) -> Vec<(Vec<Ternary>, Ternary)> {
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

#[test]
fn too_many_vars_is_rejected_before_exponential_enumeration() {
    // M16：变量数 > MAX_SOP_VARS ⇒ 3^n 枚举前就拒绝（不跑 3^13 循环、不 panic）。
    let row = vec![Ternary::False; 13];
    let err = minimize(&[(row, Ternary::True)]).unwrap_err();
    assert!(matches!(err, CodecError::CubeTooLarge { n: 13, max: 12 }));
}
