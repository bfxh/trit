//! `Xor2`（Verilog 经典 XOR）的常量恒等式与格对偶：`(p ∨ q) ^ (p ∧ q) = p ^ q`。

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
    if let Some((p, q)) = super::lattice::pair::meet_join(kept, left, right) {
        push_node(resolve, kept, consts, Op::Xor2(p, q));
        return;
    }
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
