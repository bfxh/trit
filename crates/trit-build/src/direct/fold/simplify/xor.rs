//! 三值强加 `⊕` 的常量律，按代数分派。
//!
//! K3/B3 的 `⊕` 在已知值上按合取：`x ⊕ 1 = x`、`x ⊕ X = X`；`0` 不是单位元。
//! Ł3 的 `⊕` 是饱和加法：`x ⊕ 0 = x`、`x ⊕ 1 = 1`；`X` 处 `F ⊕ X` 与 `X ⊕ X` 不同，保留原式。
//! `x ⊕ ¬x = 1` 只在 Ł3 成立；K3/B3 的 `X ⊕ X = X` 明确保留。

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
