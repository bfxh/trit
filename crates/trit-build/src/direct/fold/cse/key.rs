//! CSE 的键与操作数重映射：`(算子, 左, 右)` 三元键，交换律算子先排序。

use trit_core::value::Ternary;

use crate::direct::ir::Op;

pub(super) type Key = (u8, usize, usize);

pub(super) fn remap_op(op: Op, map: &[usize]) -> Op {
    match op {
        Op::Const(value) => Op::Const(value),
        Op::Var(slot) => Op::Var(slot),
        Op::Not(inner) => Op::Not(map[inner]),
        Op::And(left, right) => Op::And(map[left], map[right]),
        Op::Or(left, right) => Op::Or(map[left], map[right]),
        Op::Xor(left, right) => Op::Xor(map[left], map[right]),
        Op::Xor2(left, right) => Op::Xor2(map[left], map[right]),
        Op::Implies(left, right) => Op::Implies(map[left], map[right]),
        Op::Equiv(left, right) => Op::Equiv(map[left], map[right]),
    }
}

pub(super) fn normalize(op: Op) -> Op {
    match op {
        Op::Xor(a, b) => Op::Xor(a.min(b), a.max(b)),
        Op::Xor2(a, b) => Op::Xor2(a.min(b), a.max(b)),
        Op::Equiv(a, b) => Op::Equiv(a.min(b), a.max(b)),
        other => other,
    }
}

pub(super) fn key_of(op: Op) -> Key {
    match op {
        Op::Const(Ternary::False) => (0, 0, 0),
        Op::Const(Ternary::Unknown) => (0, 1, 0),
        Op::Const(Ternary::True) => (0, 2, 0),
        Op::Var(slot) => (1, slot, 0),
        Op::Not(inner) => (2, inner, 0),
        Op::And(..) | Op::Or(..) => unreachable!("And/Or 走结合链"),
        Op::Xor(left, right) => (5, left, right),
        Op::Xor2(left, right) => (6, left, right),
        Op::Implies(left, right) => (7, left, right),
        Op::Equiv(left, right) => (8, left, right),
    }
}
