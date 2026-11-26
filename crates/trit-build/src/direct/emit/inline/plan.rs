//! 内联决策：单用节点从最近的发射父节点起，最多并进 3 层。
//!
//! 这里不再只沿 `Not` 的操作数锥走：任何只被引用一次的节点都可以不落临时变量，
//! 由父语句的 C 表达式递归展开。扇出节点（`uses > 1`）仍单独发语句；根节点
//! 天然是发射点。深度封顶避免一条语句被单用长链撑成超长表达式。

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
    if uses[index] != 1 || inlined[index] {
        return;
    }
    if depth > MAX_INLINE_DEPTH {
        // 到边界就不再并进上一层；该节点自己发射，子树重新计深度。
        mark_children(uses, inlined, ops, &ops[index], 1);
        return;
    }
    inlined[index] = true;
    mark_children(uses, inlined, ops, &ops[index], depth + 1);
}
