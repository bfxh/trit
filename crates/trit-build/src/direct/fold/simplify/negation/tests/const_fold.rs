//! `Not(Const)` 在化简内即时折成常量：父节点本遍就能看到毒值，不必等后续 pass 补。

use crate::direct::fold::fold;
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::value::Ternary;

#[test]
fn constant_not_feeds_poison_into_the_parent() {
    let ops = vec![
        Op::Var(0),
        Op::Const(Ternary::Unknown),
        Op::And(0, 1),
        Op::Not(2),
        Op::Var(1),
        Op::And(3, 4),
    ];
    assert_eq!(
        fold(ops.clone(), Algebra::Bochvar),
        vec![Op::Const(Ternary::Unknown)]
    );
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        let out = fold(ops.clone(), alg);
        assert!(matches!(out.last(), Some(Op::And(..))), "{alg:?}: {out:?}");
    }
}
