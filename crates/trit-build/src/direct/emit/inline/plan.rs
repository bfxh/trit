//! 内联决策：单用节点从最近的发射父节点起沿单用边并进父表达式，不再封深度。
//! 变量叶子复用 ≤5 次直接重写（≥6 次临时变量更短），常量叶子总是直接重写；扇出节点单独发语句。
use crate::direct::ir::Op;
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
    let mut stack = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        // 只从会发射的节点起算；单用节点会在某个发射父节点的表达式里展开。
        if uses[index] == 1 {
            continue;
        }
        stack.clear();
        push_children(op, &mut stack);
        while let Some(index) = stack.pop() {
            if matches!(ops[index], Op::Var(_) | Op::Const(_)) {
                if matches!(ops[index], Op::Const(_)) || uses[index] <= 5 {
                    inlined[index] = true;
                }
                continue;
            }
            if uses[index] != 1 || inlined[index] {
                continue;
            }
            inlined[index] = true;
            push_children(&ops[index], &mut stack);
        }
    }
    inlined
}
fn push_children(op: &Op, stack: &mut Vec<usize>) {
    match *op {
        Op::Const(_) | Op::Var(_) => {}
        Op::Not(inner) => stack.push(inner),
        Op::And(left, right)
        | Op::Or(left, right)
        | Op::Xor(left, right)
        | Op::Xor2(left, right)
        | Op::Implies(left, right)
        | Op::Equiv(left, right) => {
            stack.push(left);
            stack.push(right);
        }
    }
}
