//! 结合链判据：交换/结合 3×3×3 表、镜像结合共用一条链、重复叶子去重。

use super::super::cse;
use crate::direct::ir::Op;
use trit_core::Algebra;

mod chain;
mod dual;
const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [trit_core::value::Ternary; 3] = [
    trit_core::value::Ternary::False,
    trit_core::value::Ternary::Unknown,
    trit_core::value::Ternary::True,
];

#[test]
fn associativity_holds_for_every_algebra_and_triple() {
    for alg in ALGS {
        for a in VALUES {
            for b in VALUES {
                for c in VALUES {
                    assert_eq!(
                        alg.and(alg.and(a, b), c),
                        alg.and(a, alg.and(b, c)),
                        "{alg:?} {a:?} {b:?} {c:?}"
                    );
                    assert_eq!(
                        alg.or(alg.or(a, b), c),
                        alg.or(a, alg.or(b, c)),
                        "{alg:?} {a:?} {b:?} {c:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn mirrored_associations_share_one_chain() {
    let out = cse(vec![
        Op::Var(0),
        Op::Var(1),
        Op::Var(2),
        Op::And(1, 2),
        Op::And(3, 0),
        Op::And(1, 2),
        Op::And(0, 5),
        Op::Or(4, 6),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Var(2),
            Op::And(0, 1),
            Op::And(3, 2)
        ]
    );
}

#[test]
fn duplicate_leaves_collapse_by_idempotence() {
    let out = cse(vec![Op::Var(0), Op::Var(1), Op::And(0, 1), Op::And(0, 2)]);
    assert_eq!(out, vec![Op::Var(0), Op::Var(1), Op::And(0, 1)]);
}
