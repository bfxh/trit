//! 化简后的常量回折：化简会把子节点塌成 `Const`，父节点可能错过第一遍折叠。
//! 这里按 `Algebra` 再算一遍所有操作数都是常量的节点，最后统一 DCE；语义与 `fold` 同源。

use trit_circuit::GateKind;
use trit_core::Algebra;
use trit_core::value::Ternary;

use super::super::ir::Op;
use super::super::lower::prune_dead;

pub(super) fn refold(ops: Vec<Op>, alg: Algebra) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let mut values: Vec<Option<Ternary>> = Vec::with_capacity(ops.len());
    let mut kept: Vec<Op> = Vec::with_capacity(ops.len());
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
    prune_dead(kept, root)
}

#[cfg(test)]
mod tests;
