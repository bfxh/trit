//! `Or` 结合链必须保持对偶算子，不能被写成 `And`。

use super::super::super::cse;
use crate::direct::ir::Op;

#[test]
fn or_chain_keeps_dual_operator() {
    let out = cse(vec![
        Op::Var(0),
        Op::Var(1),
        Op::Var(2),
        Op::Or(0, 1),
        Op::Or(3, 2),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Var(2),
            Op::Or(0, 1),
            Op::Or(3, 2)
        ]
    );
}
