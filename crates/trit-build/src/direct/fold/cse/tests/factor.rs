//! 公因子提取判据：两侧同算子节点有直接相同操作数时，合成
//! `a ∧ (b ∨ c)` / `a ∨ (b ∧ c)`；不同操作数原样保留。

use super::super::cse;
use crate::direct::ir::Op;

#[test]
fn common_and_factors_out_of_or() {
    let out = cse(vec![
        Op::Var(0),
        Op::Var(1),
        Op::And(0, 1),
        Op::Var(2),
        Op::And(0, 3),
        Op::Or(2, 4),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Var(2),
            Op::Or(1, 2),
            Op::And(0, 3)
        ]
    );
}

#[test]
fn common_or_factors_out_of_and() {
    let out = cse(vec![
        Op::Var(0),
        Op::Var(1),
        Op::Or(0, 1),
        Op::Var(2),
        Op::Or(0, 3),
        Op::And(2, 4),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Var(2),
            Op::And(1, 2),
            Op::Or(0, 3)
        ]
    );
}

#[test]
fn common_second_operand_factors() {
    let out = cse(vec![
        Op::Var(0),
        Op::Var(1),
        Op::Var(2),
        Op::And(1, 2),
        Op::And(0, 2),
        Op::Or(3, 4),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Var(2),
            Op::Or(0, 1),
            Op::And(2, 3)
        ]
    );
}

#[test]
fn disjoint_operands_stay() {
    let out = cse(vec![
        Op::Var(0),
        Op::Var(1),
        Op::And(0, 1),
        Op::Var(2),
        Op::Var(3),
        Op::And(3, 4),
        Op::Or(2, 5),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::And(0, 1),
            Op::Var(2),
            Op::Var(3),
            Op::And(3, 4),
            Op::Or(2, 5)
        ]
    );
}
