//! `And` / `Or` 的化简分支：常量吸收、格吸收、B3 补元/毒值吸收、单位律与幂等律；按代数分派。

use super::build::{push_const, push_node};
use super::complement;
use super::lattice;
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::value::Ternary;

pub(super) fn and(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
    alg: Algebra,
) {
    let (left, right) = (resolve[left], resolve[right]);
    if let Some(value) = poisoned(alg, consts[left], consts[right]) {
        push_const(resolve, kept, consts, value);
    } else if !matches!(alg, Algebra::Bochvar)
        && (consts[left] == Some(Ternary::False) || consts[right] == Some(Ternary::False))
    {
        push_const(resolve, kept, consts, Ternary::False);
    } else if let Some(alias) = lattice::absorbed(alg, kept, left, right, true) {
        resolve.push(alias);
    } else if left == right || consts[left] == Some(Ternary::True) {
        resolve.push(right);
    } else if consts[right] == Some(Ternary::True) {
        resolve.push(left);
    } else if let Some((outer, other)) = complement::pair(alg, kept, left, right, true) {
        push_pair(resolve, kept, consts, outer, other, true);
    } else {
        push_node(resolve, kept, consts, Op::And(left, right));
    }
}

pub(super) fn or(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
    alg: Algebra,
) {
    let (left, right) = (resolve[left], resolve[right]);
    if let Some(value) = poisoned(alg, consts[left], consts[right]) {
        push_const(resolve, kept, consts, value);
    } else if !matches!(alg, Algebra::Bochvar)
        && (consts[left] == Some(Ternary::True) || consts[right] == Some(Ternary::True))
    {
        push_const(resolve, kept, consts, Ternary::True);
    } else if let Some(alias) = lattice::absorbed(alg, kept, left, right, false) {
        resolve.push(alias);
    } else if left == right || consts[left] == Some(Ternary::False) {
        resolve.push(right);
    } else if consts[right] == Some(Ternary::False) {
        resolve.push(left);
    } else if let Some((outer, other)) = complement::pair(alg, kept, left, right, false) {
        push_pair(resolve, kept, consts, outer, other, false);
    } else {
        push_node(resolve, kept, consts, Op::Or(left, right));
    }
}

fn push_pair(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    other: usize,
    and: bool,
) {
    if left == other {
        resolve.push(left);
    } else {
        let op = if and {
            Op::And(left, other)
        } else {
            Op::Or(left, other)
        };
        push_node(resolve, kept, consts, op);
    }
}
/// B3 的 `U` 是毒值：`x ∧ U = U`、`x ∨ U = U`；K3/Ł3 的反例是
/// `F ∧ U = F`、`T ∨ U = T`，所以只在 B3 开闸。
fn poisoned(alg: Algebra, left: Option<Ternary>, right: Option<Ternary>) -> Option<Ternary> {
    (matches!(alg, Algebra::Bochvar)
        && (left == Some(Ternary::Unknown) || right == Some(Ternary::Unknown)))
    .then_some(Ternary::Unknown)
}
