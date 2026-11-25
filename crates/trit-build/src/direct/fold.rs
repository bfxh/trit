//! 常量折叠 + 常量传播：把「操作数全是常量」的节点换成 `Op::Const`。
//!
//! 语义只走 `Algebra`（`Xor2` 走 `GateKind::XorClassic.eval`），不手写代数恒等式：
//! `x & 0 = 0` 这类规则在三种代数下虽然成立，但手写一次就要再证一次；
//! 这里只做「有常量就现算」，算不出常量就原样保留。
//!
//! 折叠后仍保留每条指令的下标（只替换算子，不删除节点），最后用
//! `prune_dead` 统一收孤儿。这样根恒为最后一条，渲染器不用改。

use trit_circuit::GateKind;
use trit_core::Algebra;
use trit_core::value::Ternary;

use super::ir::Op;
use super::lower::prune_dead;
mod simplify;
pub(super) fn fold(ops: Vec<Op>, alg: Algebra) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let mut values: Vec<Option<Ternary>> = Vec::with_capacity(ops.len());
    let mut kept = Vec::with_capacity(ops.len());
    for op in ops {
        let value = match op {
            Op::Const(value) => Some(value),
            Op::Var(_) => None,
            Op::Not(inner) => values[inner].map(|value| alg.not(value)),
            Op::And(left, right) => values[left].zip(values[right]).map(|(a, b)| alg.and(a, b)),
            Op::Or(left, right) => values[left].zip(values[right]).map(|(a, b)| alg.or(a, b)),
            Op::Xor(left, right) => values[left]
                .zip(values[right])
                .map(|(a, b)| alg.strong_add(a, b)),
            Op::Xor2(left, right) => values[left]
                .zip(values[right])
                .map(|(a, b)| GateKind::XorClassic.eval(&[a, b], alg)),
            Op::Implies(left, right) => values[left]
                .zip(values[right])
                .map(|(a, b)| alg.implies(a, b)),
            Op::Equiv(left, right) => values[left]
                .zip(values[right])
                .map(|(a, b)| alg.equiv(a, b)),
        };
        let out = match value {
            Some(value) => Op::Const(value),
            None => op,
        };
        values.push(value);
        kept.push(out);
    }
    let root = kept.len() - 1;
    simplify::simplify(prune_dead(kept, root), alg)
}

#[cfg(test)]
mod tests;
