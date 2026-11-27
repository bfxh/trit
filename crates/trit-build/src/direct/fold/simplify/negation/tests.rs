//! 否定归一判据：德摩根 3×3 表 oracle、双重否定与两侧取反的形状、单侧取反保留。

use crate::direct::emit;
use crate::direct::fold::fold;
use crate::direct::ir::Op;
use trit_core::value::Ternary;
use trit_core::{Algebra, Parser};

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];
mod const_emit;
mod const_fold;
#[test]
fn de_morgan_holds_for_every_algebra_and_value_pair() {
    for alg in ALGS {
        for a in VALUES {
            for b in VALUES {
                assert_eq!(
                    alg.not(alg.and(a, b)),
                    alg.or(alg.not(a), alg.not(b)),
                    "{alg:?} {a:?} {b:?}"
                );
                assert_eq!(
                    alg.not(alg.or(a, b)),
                    alg.and(alg.not(a), alg.not(b)),
                    "{alg:?} {a:?} {b:?}"
                );
            }
        }
    }
}
#[test]
fn double_negation_aliases_and_one_sided_inversion_stays() {
    assert_eq!(
        fold(vec![Op::Var(0), Op::Not(0), Op::Not(1)], ALGS[0]),
        vec![Op::Var(0)]
    );
    let out = fold(
        vec![
            Op::Var(0),
            Op::Not(0),
            Op::Var(1),
            Op::And(1, 2),
            Op::Not(3),
        ],
        ALGS[0],
    );
    assert!(matches!(out.last(), Some(Op::Not(..))), "{out:?}");
}

#[test]
fn both_inverted_operands_collapse_to_the_dual_operator() {
    for alg in ALGS {
        let and = fold(
            vec![
                Op::Var(0),
                Op::Not(0),
                Op::Var(1),
                Op::Not(2),
                Op::And(1, 3),
                Op::Not(4),
            ],
            alg,
        );
        assert_eq!(and, vec![Op::Var(0), Op::Var(1), Op::Or(0, 1)], "{alg:?}");
        let or = fold(
            vec![
                Op::Var(0),
                Op::Not(0),
                Op::Var(1),
                Op::Not(2),
                Op::Or(1, 3),
                Op::Not(4),
            ],
            alg,
        );
        assert_eq!(or, vec![Op::Var(0), Op::Var(1), Op::And(0, 1)], "{alg:?}");
    }
}

#[test]
fn render_sheds_the_two_inner_negations_and_the_outer_one() {
    let expr = Parser::parse("!(!a & !b)").expect("解析");
    let vars = expr.variables();
    let c = emit(&expr, &vars, Algebra::Kleene).expect("生成");
    assert_eq!(c.matches("int t").count(), 0, "{c}");
    assert!(!c.contains("T_NOT"), "{c}");
    assert!(c.contains("T["), "{c}");
}
