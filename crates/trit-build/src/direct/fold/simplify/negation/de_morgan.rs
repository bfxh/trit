//! 德摩根回收：`¬(¬a ∧ ¬b) = a ∨ b`、`¬(¬a ∨ ¬b) = a ∧ b` 三代数通用。
//! 只有两侧都带 `Not` 才回收；单侧取反新建 `Not` 不减少节点，留在 `negation` 主线。

use crate::direct::ir::Op;

pub(super) fn de_morgan(kept: &[Op], inner: usize) -> Option<Op> {
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
