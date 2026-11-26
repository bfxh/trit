//! 取反对偶与 `⇔ F` 的判据：表对拍 + 化简形状 + 生成 C 不注册多余表。

use crate::direct::emit;
use crate::direct::fold::fold;
use crate::direct::ir::Op;
use trit_core::value::Ternary;
use trit_core::{Algebra, Parser};

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

#[test]
fn negated_operand_laws_match_the_tables() {
    for alg in ALGS {
        for x in VALUES {
            for y in VALUES {
                let implies = alg.implies(alg.not(x), alg.not(y));
                assert_eq!(implies, alg.implies(y, x), "{alg:?}");
                let equiv = alg.equiv(alg.not(x), alg.not(y));
                assert_eq!(equiv, alg.equiv(x, y), "{alg:?}");
                assert_eq!(alg.implies(x, Ternary::False), alg.not(x), "{alg:?}");
                assert_eq!(alg.equiv(x, Ternary::False), alg.not(x), "{alg:?}");
                assert_eq!(alg.equiv(Ternary::False, x), alg.not(x), "{alg:?}");
            }
        }
    }
}

#[test]
fn false_equivalence_negates_the_other_operand() {
    for alg in ALGS {
        for (left, right) in [(0, 1), (1, 0)] {
            let out = fold(
                vec![
                    Op::Var(0),
                    Op::Const(Ternary::False),
                    Op::Equiv(left, right),
                ],
                alg,
            );
            assert!(matches!(out.last(), Some(Op::Not(_))), "{alg:?}");
            assert!(!out.iter().any(|op| matches!(op, Op::Equiv(..))), "{alg:?}");
        }
    }
}

#[test]
fn double_negated_operands_cancel() {
    for alg in ALGS {
        let out = fold(
            vec![
                Op::Var(0),
                Op::Var(1),
                Op::Not(0),
                Op::Not(1),
                Op::Implies(2, 3),
            ],
            alg,
        );
        assert!(!out.iter().any(|op| matches!(op, Op::Not(_))), "{alg:?}");
        assert!(
            matches!(out.last(), Some(Op::Implies(a, b)) if *a == 1 && *b == 0),
            "{alg:?}: {out:?}"
        );
        let out = fold(
            vec![
                Op::Var(0),
                Op::Var(1),
                Op::Not(0),
                Op::Not(1),
                Op::Equiv(2, 3),
            ],
            alg,
        );
        assert!(
            !out.iter().any(|op| matches!(op, Op::Not(_))),
            "{alg:?}: {out:?}"
        );
        assert!(
            matches!(out.last(), Some(Op::Equiv(a, b)) if *a == 0 && *b == 1),
            "{alg:?}: {out:?}"
        );
    }
}

#[test]
fn false_equivalence_emits_one_return_without_tables() {
    for alg in ALGS {
        let expr = Parser::parse("a <=> 0").expect("解析");
        let vars = expr.variables();
        let c = emit(&expr, &vars, alg).expect("生成");
        assert_eq!(c.matches("    int8_t t").count(), 0, "{alg:?}: {c}");
        assert!(!c.contains("static const int8_t T_"), "{alg:?}: {c}");
        assert!(c.contains("return -(vars[0]);"), "{alg:?}: {c}");
    }
}

#[test]
fn double_negated_operands_emit_without_extra_minus() {
    for (source, expected) in [
        ("(!a) => (!b)", "T_IMPL[3*vars[1]+vars[0]+4]"),
        ("(!a) <=> (!b)", "T_EQUIV[3*vars[0]+vars[1]+4]"),
    ] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let c = emit(&expr, &vars, Algebra::Kleene).expect("生成");
        assert!(c.contains(expected), "{source}: {c}");
        assert!(!c.contains("-(vars"), "{source}: {c}");
        assert!(!c.contains("trit_bin("), "{source}: {c}");
    }
}

#[test]
fn same_negated_equivalence_still_folds_only_in_lukasiewicz() {
    for alg in [Algebra::Kleene, Algebra::Bochvar] {
        let out = fold(vec![Op::Var(0), Op::Not(0), Op::Equiv(1, 1)], alg);
        assert_eq!(out, vec![Op::Var(0), Op::Equiv(0, 0)], "{alg:?}");
    }
    assert_eq!(
        fold(
            vec![Op::Var(0), Op::Not(0), Op::Equiv(1, 1)],
            Algebra::Lukasiewicz
        ),
        vec![Op::Const(Ternary::True)]
    );
}
