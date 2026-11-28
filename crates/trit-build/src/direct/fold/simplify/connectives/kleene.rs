//! K3 的 `a⇒b = ¬a∨b`：复用 `Or` 表，不再单独登记 `Implies` 表。
use super::super::{absorb, negation};
use crate::direct::ir::Op;
use trit_core::{Algebra, value::Ternary};
pub(super) fn implies(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
) {
    let base = resolve.len();
    negation::push(resolve, kept, consts, left);
    let left = *resolve.last().expect("非空 IR");
    absorb::or(resolve, kept, consts, left, right, Algebra::Kleene);
    let out = *resolve.last().expect("非空 IR");
    resolve.truncate(base);
    resolve.push(out);
}
