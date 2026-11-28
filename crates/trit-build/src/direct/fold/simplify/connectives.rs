//! `⇒` / `⇔` 的常量律与形状规则：只做能真正减少节点的规则，按代数分派。
//! K3/Ł3 的 `x ⇒ T = T`、`F ⇒ x = T`；B3 在 `X ⇒ T`、`F ⇒ X` 上返回 `U`，不做。
//! Ł3 的 `x ⇒ x = T`、`x ⇔ x = T`；K3/B3 保留原式。取反对偶与 `⇔ F` 在 `negated`。
use super::build::{push_const, push_node};
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::value::Ternary;
mod kleene;
mod negated;
pub(super) fn implies(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    raw_left: usize,
    raw_right: usize,
    alg: Algebra,
) {
    let (left, right) = (resolve[raw_left], resolve[raw_right]);
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
        && (negated::is_not_of(kept, right, left) || negated::is_not_of(kept, left, right))
    {
        resolve.push(right);
        return;
    }
    if matches!(alg, Algebra::Kleene) {
        kleene::implies(resolve, kept, consts, raw_left, raw_right);
        return;
    }
    if negated::implies(resolve, kept, consts, left, right) {
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
    if negated::equiv(resolve, kept, consts, left, right, alg) {
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
#[cfg(test)]
mod tests;
