//! 反向分配（提取公因子）：`(a ∧ b) ∨ (a ∧ c) = a ∧ (b ∨ c)`，对偶
//! `(a ∨ b) ∧ (a ∨ c) = a ∨ (b ∧ c)`。`∧` / `∨` 在三代数下都是分配格，
//! 不依赖 `¬` 或排中律；只在两侧同算子节点有直接相同的操作数时命中，
//! 子式里埋着的公因子不猜。
//!
//! CSE 先跑一遍把同构节点合并，因此这里的操作数下标相同就代表同一条子式；
//! 命中后新建内层与外层节点，再交给下一遍 CSE 压平/去重。

use crate::direct::ir::Op;

use super::key::remap_op;

pub(super) fn factor(ops: Vec<Op>) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let mut remap = vec![usize::MAX; ops.len()];
    let mut kept = Vec::with_capacity(ops.len());
    for (index, op) in ops.into_iter().enumerate() {
        let op = remap_op(op, &remap);
        match op {
            Op::Or(left, right) => match common(kept.as_slice(), left, right, true) {
                Some((shared, a, b)) => {
                    let inner = push(&mut kept, Op::Or(a, b));
                    remap[index] = push(&mut kept, Op::And(shared, inner));
                }
                None => remap[index] = push(&mut kept, Op::Or(left, right)),
            },
            Op::And(left, right) => match common(kept.as_slice(), left, right, false) {
                Some((shared, a, b)) => {
                    let inner = push(&mut kept, Op::And(a, b));
                    remap[index] = push(&mut kept, Op::Or(shared, inner));
                }
                None => remap[index] = push(&mut kept, Op::And(left, right)),
            },
            other => remap[index] = push(&mut kept, other),
        }
    }
    crate::direct::lower::prune_dead(kept, remap[remap.len() - 1])
}

/// `outer_or` 为真时外层是 `Or`、两侧必须是 `And`；为假时镜像。
fn common(kept: &[Op], left: usize, right: usize, outer_or: bool) -> Option<(usize, usize, usize)> {
    let (l1, l2) = operands(kept, left, outer_or)?;
    let (r1, r2) = operands(kept, right, outer_or)?;
    for (shared, other) in [(l1, l2), (l2, l1)] {
        for (candidate, other_right) in [(r1, r2), (r2, r1)] {
            if shared == candidate
                && other != other_right
                && other != shared
                && other_right != shared
            {
                return Some((shared, other, other_right));
            }
        }
    }
    None
}

fn operands(kept: &[Op], index: usize, inner_or: bool) -> Option<(usize, usize)> {
    match (inner_or, kept[index]) {
        (true, Op::And(a, b)) | (false, Op::Or(a, b)) => Some((a, b)),
        _ => None,
    }
}

fn push(kept: &mut Vec<Op>, op: Op) -> usize {
    kept.push(op);
    kept.len() - 1
}
