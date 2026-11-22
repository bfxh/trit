//! 三值真值表与准入门的组合矩阵。
//!
//! 手写组合测试的通病是"只测到想得到的那些格子"。这里用**全表**：K3 AND 的 9 格、
//! 依赖门的 9 格、四门准入的 81 格全部走一遍。改错一格就红，而且失败信息直接给出
//! 是哪个输入组合坏了 —— 三值逻辑的 bug 几乎都是"某一格悄悄变成了 False/True"。

use trit_orch::{EvidenceVerdict, Ternary, admit, admit_with_claim, dependency_gate};

const ALL: [Ternary; 3] = [Ternary::True, Ternary::Unknown, Ternary::False];

#[test]
fn k3_and_truth_table_is_exact() {
    use Ternary::{False, True as T, Unknown as U};
    // 行 = 左操作数，列 = 右操作数。False 支配，True 只与 True 得 True。
    let expected = [
        [T, U, False], // T & {T,U,F}
        [U, U, False], // U & {T,U,F}
        [False, False, False],
    ];
    for (left_index, left) in ALL.into_iter().enumerate() {
        for (right_index, right) in ALL.into_iter().enumerate() {
            assert_eq!(
                left & right,
                expected[left_index][right_index],
                "ORCH_TERNARY_AND 格 ({left:?} & {right:?})"
            );
        }
    }
}

#[test]
fn k3_and_is_commutative_on_every_cell() {
    for left in ALL {
        for right in ALL {
            assert_eq!(left & right, right & left, "ORCH_TERNARY_COMMUTATIVE");
        }
    }
}

#[test]
fn k3_and_is_associative_on_every_cell() {
    // 结合律决定"先算哪两门"不影响结论；破了它，门顺序就会变成结论的一部分。
    for a in ALL {
        for b in ALL {
            for c in ALL {
                assert_eq!(
                    (a & b) & c,
                    a & (b & c),
                    "ORCH_TERNARY_ASSOCIATIVE ({a:?},{b:?},{c:?})"
                );
            }
        }
    }
}

#[test]
fn k3_or_dual_holds_on_every_cell() {
    // De Morgan 对偶：¬(a ∧ b) = ¬a ∨ ¬b。AND 改错时，这条常能抓到 OR 侧的不对称。
    let not = |value: Ternary| match value {
        Ternary::True => Ternary::False,
        Ternary::False => Ternary::True,
        Ternary::Unknown => Ternary::Unknown,
    };
    for a in ALL {
        for b in ALL {
            assert_eq!(!(a & b), not(a) | not(b), "ORCH_TERNARY_DUAL");
        }
    }
}

#[test]
fn unknown_and_false_are_both_refusals_but_mean_different_things() {
    // 这条是给读日志的人看的：两种"不准"在值上都不能解锁，但出口不同。
    for gate in [Ternary::Unknown, Ternary::False] {
        assert_ne!(gate, Ternary::True, "ORCH_TERNARY_NO_UNLOCK");
    }
    assert_ne!(
        Ternary::Unknown,
        Ternary::False,
        "Unknown 不是 False：一个补证，一个放弃"
    );
}

#[test]
fn dependency_gate_table() {
    // required=False → 这个依赖不存在，直接放行；required=Unknown → 需求未定，只能未知。
    use Ternary::{False, True as T, Unknown as U};
    let expected = [
        // required = True, Unknown, False × evidence = True, Unknown, False
        [T, U, False], // required T：结论就是证据
        [U, U, U],     // required U：证据如何都不重要，需求本身没定
        [T, T, T],     // required F：依赖不需要，直接放行
    ];
    for (r_index, required) in ALL.into_iter().enumerate() {
        for (e_index, evidence) in ALL.into_iter().enumerate() {
            assert_eq!(
                dependency_gate(required, evidence),
                expected[r_index][e_index],
                "ORCH_GATE_DEP ({required:?},{evidence:?})"
            );
        }
    }
}

#[test]
fn dependency_gate_never_escalates_unknown_to_true() {
    for evidence in ALL {
        assert_ne!(
            dependency_gate(Ternary::Unknown, evidence),
            Ternary::True,
            "ORCH_GATE_NO_ESCALATION"
        );
    }
}

#[test]
fn admission_covers_all_eighty_one_combinations() {
    // 四门 3^4 = 81 种组合。任何一格判 True 都意味着一个本该被拦的批次被放行。
    let mut answers = std::collections::BTreeMap::new();
    for dependencies in ALL {
        for authorization in ALL {
            for resources in ALL {
                for freshness in ALL {
                    let verdict = admit(dependencies, authorization, resources, freshness);
                    let expected = if [dependencies, authorization, resources, freshness]
                        .iter()
                        .all(|gate| *gate == Ternary::True)
                    {
                        Ternary::True
                    } else if [dependencies, authorization, resources, freshness]
                        .contains(&Ternary::False)
                    {
                        Ternary::False
                    } else {
                        Ternary::Unknown
                    };
                    assert_eq!(
                        verdict, expected,
                        "ORCH_ADMIT_MATRIX ({dependencies:?},{authorization:?},{resources:?},{freshness:?})"
                    );
                    *answers.entry(format!("{verdict:?}")).or_insert(0usize) += 1;
                }
            }
        }
    }
    // 81 = 3^4：全 True 只有 1 格；含至少一个 False 是 81 - 2^4 = 65 格；
    // 剩下 81 - 1 - 65 = 15 格是"没有 False 但有 Unknown"。
    assert_eq!(answers["True"], 1, "ORCH_ADMIT_COUNT_T");
    assert_eq!(answers["False"], 65, "ORCH_ADMIT_COUNT_F");
    assert_eq!(answers["Unknown"], 15, "ORCH_ADMIT_COUNT_U");
}

#[test]
fn a_fifth_gate_is_not_weaker_than_four() {
    // 加门只能收紧。若第五门是 True 就与四门同结论，否则必须更严。
    for four in [Ternary::True, Ternary::Unknown, Ternary::False] {
        for claim in ALL {
            let with_claim = admit_with_claim(four, four, four, four, claim);
            let without = admit(four, four, four, four);
            assert!(
                with_claim != Ternary::True || without == Ternary::True && claim == Ternary::True,
                "ORCH_ADMIT_CLAIM_TIGHTEN"
            );
        }
    }
}

#[test]
fn evidence_verdicts_fold_into_ternary_without_losing_conflicts() {
    assert_eq!(EvidenceVerdict::Proved.value(), Ternary::True);
    assert_eq!(EvidenceVerdict::Refuted.value(), Ternary::False);
    // 冲突与缺失都必须落成 Unknown：它们都不能解锁下游。
    assert_eq!(EvidenceVerdict::Conflicted.value(), Ternary::Unknown);
    assert_eq!(EvidenceVerdict::Neither.value(), Ternary::Unknown);
    assert!(EvidenceVerdict::Conflicted.is_conflicted());
    assert!(!EvidenceVerdict::Neither.is_conflicted());
}

#[test]
fn caps_are_capped() {
    // 三值运算符不许出现第四种答案：任何组合的结果都必须仍属于这三个值之一。
    for left in ALL {
        for right in ALL {
            let results = [left & right, left | right, dependency_gate(left, right)];
            for result in results {
                assert!(
                    matches!(result, Ternary::True | Ternary::Unknown | Ternary::False),
                    "ORCH_TERNARY_CLOSED"
                );
            }
        }
    }
}
