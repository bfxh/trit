//! 否定归一：双重否定消去 + 德摩根回收。
//!
//! `¬¬x = x` 三代数通用。`¬(¬a ∧ ¬b) = a ∨ b`、`¬(¬a ∨ ¬b) = a ∧ b`
//! 也三代数通用：K3/Ł3 的 `∧`/`∨` 是格序 min/max，B3 的 U 传染在两侧同值。
//! 只回收「两个操作数都已经是 `Not`」的形状，节点数 4→1；单侧取反仍要新建
//! `Not`，不减少节点，保留原式。

use trit_core::value::Ternary;

use super::build::push_node;
use crate::direct::ir::Op;

pub(super) fn push(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    inner: usize,
) {
    let inner = resolve[inner];
    if let Op::Not(target) = kept[inner] {
        resolve.push(target);
        return;
    }
    if let Some(op) = de_morgan(kept, inner) {
        push_node(resolve, kept, consts, op);
        return;
    }
    push_node(resolve, kept, consts, Op::Not(inner));
}

fn de_morgan(kept: &[Op], inner: usize) -> Option<Op> {
    let (left, right, dual) = match kept.get(inner)? {
        Op::And(left, right) => (left, right, false),
        Op::Or(left, right) => (left, right, true),
        _ => return None,
    };
    let left = not_operand(kept, *left)?;
    let right = not_operand(kept, *right)?;
    Some(if dual {
        Op::And(left, right)
    } else {
        Op::Or(left, right)
    })
}

fn not_operand(kept: &[Op], index: usize) -> Option<usize> {
    match kept.get(index)? {
        Op::Not(target) => Some(*target),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
