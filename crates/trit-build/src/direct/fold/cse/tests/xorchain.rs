//! `Xor2` 结合链判据：三代数 3×3×3 结合律、镜像链共享、重复叶子不去重。

use super::super::cse;
use crate::direct::ir::Op;
use trit_circuit::GateKind;
use trit_core::Algebra;
use trit_core::value::Ternary;

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

#[test]
fn xor_classic_is_associative_for_every_triple() {
    for alg in ALGS {
        for a in VALUES {
            for b in VALUES {
                for c in VALUES {
                    let left = GateKind::XorClassic.eval(&[a, b], alg);
                    let left = GateKind::XorClassic.eval(&[left, c], alg);
                    let right = GateKind::XorClassic.eval(&[b, c], alg);
                    let right = GateKind::XorClassic.eval(&[a, right], alg);
                    assert_eq!(left, right, "{alg:?} {a:?} {b:?} {c:?}");
                }
            }
        }
    }
}

#[test]
fn mirrored_xor_chains_share_one_chain() {
    let out = cse(vec![
        Op::Var(0),
        Op::Var(1),
        Op::Var(2),
        Op::Xor2(0, 1),
        Op::Xor2(3, 2),
        Op::Xor2(1, 2),
        Op::Xor2(0, 5),
        Op::Xor2(4, 6),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Var(2),
            Op::Xor2(0, 1),
            Op::Xor2(3, 2),
            Op::Xor2(4, 4)
        ]
    );
}

#[test]
fn duplicate_xor_leaves_are_not_deduplicated() {
    let out = cse(vec![Op::Var(0), Op::Xor2(0, 0)]);
    assert_eq!(out, vec![Op::Var(0), Op::Xor2(0, 0)]);
}
