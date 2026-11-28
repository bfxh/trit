//! 三值强加 `⊕` 的常量律与格对偶：`(p ∨ q) ⊕ (p ∧ q) = p ⊕ q`。

use trit_core::Algebra;
use trit_core::value::Ternary;

use super::build::{push_const, push_node};
use crate::direct::ir::Op;

pub(super) fn push(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
    alg: Algebra,
) {
    let left = resolve[left];
    let right = resolve[right];
    if let Some((p, q)) = super::lattice::pair::meet_join(kept, left, right) {
        push_node(resolve, kept, consts, Op::Xor(p, q));
        return;
    }
    if matches!(alg, Algebra::Lukasiewicz) {
        if consts[left] == Some(Ternary::False) {
            resolve.push(right);
            return;
        }
        if consts[right] == Some(Ternary::False) {
            resolve.push(left);
            return;
        }
        if consts[left] == Some(Ternary::True) || consts[right] == Some(Ternary::True) {
            push_const(resolve, kept, consts, Ternary::True);
            return;
        }
        if is_complement(kept, left, right) {
            push_const(resolve, kept, consts, Ternary::True);
            return;
        }
    } else {
        if consts[left] == Some(Ternary::True) {
            resolve.push(right);
            return;
        }
        if consts[right] == Some(Ternary::True) {
            resolve.push(left);
            return;
        }
        if consts[left] == Some(Ternary::Unknown) || consts[right] == Some(Ternary::Unknown) {
            push_const(resolve, kept, consts, Ternary::Unknown);
            return;
        }
    }
    push_node(resolve, kept, consts, Op::Xor(left, right));
}

fn is_complement(kept: &[Op], left: usize, right: usize) -> bool {
    matches!(kept[left], Op::Not(target) if target == right)
        || matches!(kept[right], Op::Not(target) if target == left)
}

#[cfg(test)]
mod tests;
