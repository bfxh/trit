//! `Op` 的临时变量下标重映射：CSE / 德摩根链重建后统一收口。

use super::Op;

impl Op {
    /// 按临时变量下标的映射表重写操作数；`Const` / `Var` 原样返回。
    pub(in crate::direct) fn remap(self, map: &[usize]) -> Op {
        match self {
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
}
