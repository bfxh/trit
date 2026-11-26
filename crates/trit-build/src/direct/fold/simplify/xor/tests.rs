//! 强加 `⊕` 常量律判据：oracle 是 `Algebra::strong_add` 的 3×3 表。

use crate::direct::emit;
use crate::direct::fold::fold;
use crate::direct::ir::Op;
use trit_core::value::Ternary;
use trit_core::{Algebra, Parser};

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

#[test]
fn strong_add_laws_hold_exactly_where_the_table_says() {
    for alg in ALGS {
        for value in VALUES {
            if matches!(alg, Algebra::Lukasiewicz) {
                assert_eq!(alg.strong_add(value, Ternary::False), value, "{alg:?}");
                assert_eq!(
                    alg.strong_add(value, Ternary::True),
                    Ternary::True,
                    "{alg:?}"
                );
                assert_eq!(
                    alg.strong_add(value, alg.not(value)),
                    Ternary::True,
                    "{alg:?}"
                );
            } else {
                assert_eq!(alg.strong_add(value, Ternary::True), value, "{alg:?}");
                assert_eq!(
                    alg.strong_add(value, Ternary::Unknown),
                    Ternary::Unknown,
                    "{alg:?}"
                );
            }
        }
    }
    // 负结果：K3/B3 的 `0` 不是单位元（`1 ⊕ 0 = 0`）；Ł3 的 `X ⊕ X = 1`。
    assert_eq!(
        Algebra::Kleene.strong_add(Ternary::True, Ternary::False),
        Ternary::False
    );
    assert_eq!(
        Algebra::Lukasiewicz.strong_add(Ternary::Unknown, Ternary::Unknown),
        Ternary::True
    );
}

#[test]
fn unit_and_absorbing_constants_follow_the_algebra() {
    for (alg, unit, absorbing) in [
        (Algebra::Kleene, Ternary::True, Ternary::Unknown),
        (Algebra::Lukasiewicz, Ternary::False, Ternary::True),
        (Algebra::Bochvar, Ternary::True, Ternary::Unknown),
    ] {
        for value in [unit, absorbing] {
            let expected = if value == unit {
                Op::Var(0)
            } else {
                Op::Const(value)
            };
            for ops in [
                vec![Op::Var(0), Op::Const(value), Op::Xor(0, 1)],
                vec![Op::Var(0), Op::Const(value), Op::Xor(1, 0)],
            ] {
                assert_eq!(fold(ops, alg), vec![expected], "{alg:?}");
            }
        }
    }
}

#[test]
fn non_identities_are_explicitly_preserved() {
    for ops in [
        vec![Op::Var(0), Op::Const(Ternary::False), Op::Xor(0, 1)],
        vec![Op::Var(0), Op::Not(0), Op::Xor(0, 1)],
    ] {
        let out = fold(ops, Algebra::Kleene);
        assert!(matches!(out.last(), Some(Op::Xor(..))), "{out:?}");
    }
    for ops in [
        vec![Op::Var(0), Op::Const(Ternary::Unknown), Op::Xor(0, 1)],
        vec![Op::Var(0), Op::Xor(0, 0)],
    ] {
        let out = fold(ops, Algebra::Lukasiewicz);
        assert!(matches!(out.last(), Some(Op::Xor(..))), "{out:?}");
    }
}

#[test]
fn lukasiewicz_complement_collapses_but_k3_and_bochvar_stay() {
    assert_eq!(
        fold(
            vec![Op::Var(0), Op::Not(0), Op::Xor(0, 1)],
            Algebra::Lukasiewicz
        ),
        vec![Op::Const(Ternary::True)]
    );
    for alg in [Algebra::Kleene, Algebra::Bochvar] {
        let out = fold(vec![Op::Var(0), Op::Not(0), Op::Xor(0, 1)], alg);
        assert!(matches!(out.last(), Some(Op::Xor(..))), "{out:?}");
    }
}

#[test]
fn strong_add_c_tables_follow_the_dispatch() {
    for (alg, source, statements, has_table) in [
        (Algebra::Kleene, "a ^ 1", 1, false),
        (Algebra::Kleene, "a ^ 0", 1, true),
        (Algebra::Lukasiewicz, "a ^ 0", 1, false),
        (Algebra::Lukasiewicz, "a ^ 1", 1, false),
    ] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let c = emit(&expr, &vars, alg).expect("生成");
        assert_eq!(
            c.matches("    int8_t t").count(),
            statements,
            "{alg:?} {source}: {c}"
        );
        assert_eq!(c.contains("T_XOR"), has_table, "{alg:?} {source}: {c}");
    }
}
