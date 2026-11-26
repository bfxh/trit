//! 内联决策：单用节点从最近的发射父节点起，最多并进 3 层。
//! 变量叶子复用 ≤5 次直接重写（≥6 次临时变量更短），常量叶子总是直接重写；扇出节点单独发语句。
use crate::direct::ir::Op;
/// 从发射父节点算起的最大内联层数。
const MAX_INLINE_DEPTH: usize = 3;
/// 标记哪些节点不单独发语句。
pub(crate) fn targets(ops: &[Op]) -> Vec<bool> {
    let mut uses = vec![0usize; ops.len()];
    for op in ops {
        match *op {
            Op::Not(inner) => uses[inner] += 1,
            Op::And(left, right)
            | Op::Or(left, right)
            | Op::Xor(left, right)
            | Op::Xor2(left, right)
            | Op::Implies(left, right)
            | Op::Equiv(left, right) => {
                uses[left] += 1;
                uses[right] += 1;
            }
            Op::Const(_) | Op::Var(_) => {}
        }
    }
    let mut inlined = vec![false; ops.len()];
    inlined[ops.len() - 1] = true;
    for (index, op) in ops.iter().enumerate() {
        // 只从会发射的节点起算；单用节点会在某个发射父节点的表达式里展开。
        if uses[index] == 1 {
            continue;
        }
        mark_children(&uses, &mut inlined, ops, op, 1);
    }
    inlined
}

fn mark_children(uses: &[usize], inlined: &mut [bool], ops: &[Op], op: &Op, depth: usize) {
    match *op {
        Op::Const(_) | Op::Var(_) => {}
        Op::Not(inner) => mark(uses, inlined, ops, inner, depth),
        Op::And(left, right)
        | Op::Or(left, right)
        | Op::Xor(left, right)
        | Op::Xor2(left, right)
        | Op::Implies(left, right)
        | Op::Equiv(left, right) => {
            mark(uses, inlined, ops, left, depth);
            mark(uses, inlined, ops, right, depth);
        }
    }
}

fn mark(uses: &[usize], inlined: &mut [bool], ops: &[Op], index: usize, depth: usize) {
    if matches!(ops[index], Op::Var(_) | Op::Const(_)) {
        if matches!(ops[index], Op::Const(_)) || uses[index] <= 5 {
            inlined[index] = true;
        }
        return;
    }
    if uses[index] != 1 || inlined[index] {
        return;
    }
    if depth > MAX_INLINE_DEPTH {
        mark_children(uses, inlined, ops, &ops[index], 1);
        return;
    }
    inlined[index] = true;
    mark_children(uses, inlined, ops, &ops[index], depth + 1);
}
