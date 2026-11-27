//! `Xor2`（Verilog 经典 XOR）的常量恒等式。
//!
//! 语义只认 `GateKind::XorClassic.eval` 的 3×3 表：`X` 吸收、布尔异或。
//! 所以 `x ^ 0 = x`、`x ^ 1 = ¬x`、`x ^ X = X` 三代数都成立；
//! `x ^ x` 不是常量（`X ^ X = X`），这里显式不做幂等。

use trit_core::value::Ternary;

use super::build::{push_const, push_node};
use crate::direct::ir::Op;

pub(super) fn push(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
) {
    let left = resolve[left];
    let right = resolve[right];
    match (consts[left], consts[right]) {
        (Some(Ternary::Unknown), _) | (_, Some(Ternary::Unknown)) => {
            push_const(resolve, kept, consts, Ternary::Unknown);
        }
        (Some(Ternary::False), _) => resolve.push(right),
        (_, Some(Ternary::False)) => resolve.push(left),
        (Some(Ternary::True), _) => negate(resolve, kept, consts, right),
        (_, Some(Ternary::True)) => negate(resolve, kept, consts, left),
        _ => push_node(resolve, kept, consts, Op::Xor2(left, right)),
    }
}

pub(super) fn negate(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    target: usize,
) {
    if let Some(value) = consts[target] {
        push_const(resolve, kept, consts, negated(value));
    } else if let Op::Not(inner) = kept[target] {
        resolve.push(inner);
    } else {
        push_node(resolve, kept, consts, Op::Not(target));
    }
}

fn negated(value: Ternary) -> Ternary {
    match value {
        Ternary::False => Ternary::True,
        Ternary::Unknown => Ternary::Unknown,
        Ternary::True => Ternary::False,
    }
}

#[cfg(test)]
mod tests;
