//! `⇒` / `⇔` 的常量律与形状规则：只做能真正减少节点的规则，按代数分派。
//!
//! K3/Ł3 的 `x ⇒ T = T`、`F ⇒ x = T`；B3 在 `X ⇒ T`、`F ⇒ X` 上返回 `U`，不做。
//! Ł3 的 `x ⇒ x = T`、`x ⇔ x = T`；K3/B3 在这两个式子上保留原式（`X` 处不同）。
//! K3/B3 的 `x ⇒ ¬x = ¬x`、`¬x ⇒ x = x`、`x ⇔ X = X`；Ł3 有反例，不做。

use trit_core::Algebra;
use trit_core::value::Ternary;

use super::build::{push_const, push_node};
use crate::direct::ir::Op;

pub(super) fn implies(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
    alg: Algebra,
) {
    let left = resolve[left];
    let right = resolve[right];
    if !matches!(alg, Algebra::Bochvar) && consts[right] == Some(Ternary::True) {
        push_const(resolve, kept, consts, Ternary::True);
        return;
    }
    if !matches!(alg, Algebra::Bochvar) && consts[left] == Some(Ternary::False) {
        push_const(resolve, kept, consts, Ternary::True);
        return;
    }
    if consts[left] == Some(Ternary::True) {
        resolve.push(right);
        return;
    }
    if left == right {
        if matches!(alg, Algebra::Lukasiewicz) {
            push_const(resolve, kept, consts, Ternary::True);
        } else {
            push_node(resolve, kept, consts, Op::Implies(left, right));
        }
        return;
    }
    if !matches!(alg, Algebra::Lukasiewicz)
        && (is_not_of(kept, right, left) || is_not_of(kept, left, right))
    {
        resolve.push(right);
        return;
    }
    push_node(resolve, kept, consts, Op::Implies(left, right));
}

pub(super) fn equiv(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
    alg: Algebra,
) {
    let left = resolve[left];
    let right = resolve[right];
    if consts[left] == Some(Ternary::True) {
        resolve.push(right);
        return;
    }
    if consts[right] == Some(Ternary::True) {
        resolve.push(left);
        return;
    }
    if !matches!(alg, Algebra::Lukasiewicz)
        && (consts[left] == Some(Ternary::Unknown) || consts[right] == Some(Ternary::Unknown))
    {
        push_const(resolve, kept, consts, Ternary::Unknown);
        return;
    }
    if left == right {
        if matches!(alg, Algebra::Lukasiewicz) {
            push_const(resolve, kept, consts, Ternary::True);
        } else {
            push_node(resolve, kept, consts, Op::Equiv(left, right));
        }
        return;
    }
    push_node(resolve, kept, consts, Op::Equiv(left, right));
}

fn is_not_of(kept: &[Op], outer: usize, inner: usize) -> bool {
    matches!(kept[outer], Op::Not(target) if target == inner)
}

#[cfg(test)]
mod tests;
