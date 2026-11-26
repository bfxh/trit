//! `And` / `Or` 的化简分支：常量吸收、格吸收、B3 补元消去、单位律与幂等律。
//!
//! 拆出主循环只为行数预算；规则本身仍按代数分派，B3 的 U 传染不会被 K3/Ł3
//! 的格吸收规则误伤。

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
    if !matches!(alg, Algebra::Bochvar)
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
        push_and(resolve, kept, consts, outer, other);
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
    if !matches!(alg, Algebra::Bochvar)
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
        push_or(resolve, kept, consts, outer, other);
    } else {
        push_node(resolve, kept, consts, Op::Or(left, right));
    }
}

fn push_and(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    other: usize,
) {
    if left == other {
        resolve.push(left);
    } else {
        push_node(resolve, kept, consts, Op::And(left, other));
    }
}

fn push_or(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    other: usize,
) {
    if left == other {
        resolve.push(left);
    } else {
        push_node(resolve, kept, consts, Op::Or(left, other));
    }
}
