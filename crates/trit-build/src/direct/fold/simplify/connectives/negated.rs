//! 取反操作数的对偶律与 `⇔ F`：能少节点的才换，按代数分派。

use super::super::build::{push_const, push_node};
use super::super::negation;
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::value::Ternary;

pub(super) fn implies(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
) -> bool {
    if consts[right] == Some(Ternary::False) {
        negation::push(resolve, kept, consts, left);
        return true;
    }
    if let (Op::Not(a), Op::Not(b)) = (kept[left], kept[right]) {
        push_node(resolve, kept, consts, Op::Implies(b, a));
        return true;
    }
    false
}

pub(super) fn equiv(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
    alg: Algebra,
) -> bool {
    if consts[left] == Some(Ternary::False) {
        negation::push(resolve, kept, consts, right);
        return true;
    }
    if consts[right] == Some(Ternary::False) {
        negation::push(resolve, kept, consts, left);
        return true;
    }
    if let (Op::Not(a), Op::Not(b)) = (kept[left], kept[right]) {
        if a == b && matches!(alg, Algebra::Lukasiewicz) {
            push_const(resolve, kept, consts, Ternary::True);
        } else {
            push_node(resolve, kept, consts, Op::Equiv(a, b));
        }
        return true;
    }
    false
}

pub(super) fn is_not_of(kept: &[Op], outer: usize, inner: usize) -> bool {
    matches!(kept[outer], Op::Not(target) if target == inner)
}
