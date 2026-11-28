//! `⇒` / `⇔` 常量律判据：oracle 是 `Algebra::implies` / `equiv` 的 3×3 表。
use crate::direct::emit;
use crate::direct::fold::fold;
use crate::direct::ir::Op;
use trit_core::value::Ternary;
use trit_core::{Algebra, Parser};
const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];
#[test]
fn implication_laws_hold_exactly_where_the_tables_say() {
    for alg in ALGS {
        for value in VALUES {
            if !matches!(alg, Algebra::Bochvar) {
                assert_eq!(alg.implies(value, Ternary::True), Ternary::True, "{alg:?}");
                assert_eq!(alg.implies(Ternary::False, value), Ternary::True, "{alg:?}");
            }
            assert_eq!(alg.implies(Ternary::True, value), value, "{alg:?}");
            if matches!(alg, Algebra::Lukasiewicz) {
                assert_eq!(alg.implies(value, value), Ternary::True, "{alg:?}");
            }
            if !matches!(alg, Algebra::Lukasiewicz) {
                assert_eq!(
                    alg.implies(value, alg.not(value)),
                    alg.not(value),
                    "{alg:?}"
                );
                assert_eq!(alg.implies(alg.not(value), value), value, "{alg:?}");
            }
        }
    }
    // 负结果：B3 的 U 毒值让这两条 K3/Ł3 规则失效。
    assert_eq!(
        Algebra::Bochvar.implies(Ternary::Unknown, Ternary::True),
        Ternary::Unknown
    );
    assert_eq!(
        Algebra::Bochvar.implies(Ternary::False, Ternary::Unknown),
        Ternary::Unknown
    );
}

#[test]
fn equivalence_laws_hold_exactly_where_the_tables_say() {
    for alg in ALGS {
        for value in VALUES {
            assert_eq!(alg.equiv(value, Ternary::True), value, "{alg:?}");
            assert_eq!(alg.equiv(Ternary::True, value), value, "{alg:?}");
            if matches!(alg, Algebra::Lukasiewicz) {
                assert_eq!(alg.equiv(value, value), Ternary::True, "{alg:?}");
            }
            if !matches!(alg, Algebra::Lukasiewicz) {
                assert_eq!(
                    alg.equiv(value, Ternary::Unknown),
                    Ternary::Unknown,
                    "{alg:?}"
                );
            }
        }
    }
    // Ł3 的 U⇔U 是 T，不是 U。
    assert_eq!(
        Algebra::Lukasiewicz.equiv(Ternary::Unknown, Ternary::Unknown),
        Ternary::True
    );
}

#[test]
fn implication_constants_alias_or_fold() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        assert_eq!(
            fold(
                vec![Op::Var(0), Op::Const(Ternary::True), Op::Implies(0, 1)],
                alg
            ),
            vec![Op::Const(Ternary::True)],
            "{alg:?}"
        );
        assert_eq!(
            fold(
                vec![Op::Var(0), Op::Const(Ternary::False), Op::Implies(1, 0)],
                alg
            ),
            vec![Op::Const(Ternary::True)],
            "{alg:?}"
        );
        assert_eq!(
            fold(
                vec![Op::Var(0), Op::Const(Ternary::True), Op::Implies(1, 0)],
                alg
            ),
            vec![Op::Var(0)],
            "{alg:?}"
        );
    }
    for alg in [Algebra::Kleene, Algebra::Bochvar] {
        assert_eq!(
            fold(vec![Op::Var(0), Op::Not(0), Op::Implies(0, 1)], alg),
            vec![Op::Var(0), Op::Not(0)],
            "{alg:?}"
        );
        assert_eq!(
            fold(vec![Op::Var(0), Op::Not(0), Op::Implies(1, 0)], alg),
            vec![Op::Var(0)],
            "{alg:?}"
        );
    }
    for ops in [
        vec![Op::Var(0), Op::Const(Ternary::True), Op::Implies(0, 1)],
        vec![Op::Var(0), Op::Const(Ternary::False), Op::Implies(1, 0)],
    ] {
        let out = fold(ops, Algebra::Bochvar);
        assert!(matches!(out.last(), Some(Op::Implies(..))), "{out:?}");
    }
}

#[test]
fn self_implication_is_constant_only_in_lukasiewicz() {
    assert_eq!(
        fold(vec![Op::Var(0), Op::Implies(0, 0)], Algebra::Lukasiewicz),
        vec![Op::Const(Ternary::True)]
    );
    for alg in [Algebra::Kleene, Algebra::Bochvar] {
        assert_eq!(
            fold(vec![Op::Var(0), Op::Implies(0, 0)], alg),
            vec![Op::Var(0), Op::Implies(0, 0)],
            "{alg:?}"
        );
    }
}

#[test]
fn equivalence_constants_alias_fold_or_stay() {
    for alg in ALGS {
        assert_eq!(
            fold(
                vec![Op::Var(0), Op::Const(Ternary::True), Op::Equiv(0, 1)],
                alg
            ),
            vec![Op::Var(0)],
            "{alg:?}"
        );
        assert_eq!(
            fold(
                vec![Op::Var(0), Op::Const(Ternary::True), Op::Equiv(1, 0)],
                alg
            ),
            vec![Op::Var(0)],
            "{alg:?}"
        );
    }
    for alg in [Algebra::Kleene, Algebra::Bochvar] {
        assert_eq!(
            fold(
                vec![Op::Var(0), Op::Const(Ternary::Unknown), Op::Equiv(0, 1)],
                alg
            ),
            vec![Op::Const(Ternary::Unknown)],
            "{alg:?}"
        );
        assert_eq!(
            fold(vec![Op::Var(0), Op::Equiv(0, 0)], alg),
            vec![Op::Var(0), Op::Equiv(0, 0)],
            "{alg:?}"
        );
    }
    assert_eq!(
        fold(vec![Op::Var(0), Op::Equiv(0, 0)], Algebra::Lukasiewicz),
        vec![Op::Const(Ternary::True)]
    );
    let out = fold(
        vec![Op::Var(0), Op::Const(Ternary::Unknown), Op::Equiv(0, 1)],
        Algebra::Lukasiewicz,
    );
    assert!(matches!(out.last(), Some(Op::Equiv(..))), "{out:?}");
}

#[test]
fn simplified_connectives_emit_one_statement_without_tables() {
    for source in ["a => 1", "1 => a", "a <=> 1"] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let c = emit(&expr, &vars, Algebra::Kleene).expect("生成");
        assert_eq!(c.matches("int t").count(), 0, "{source}: {c}");
        assert!(!c.contains("C T[]"), "{source}: {c}");
    }
}
mod kleene;
mod kleene_resolve;
mod negated;
