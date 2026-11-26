//! 取反子式的递归内联：只沿「单用」边下钻，深度封顶，扇出节点不复制。

use super::super::code;
use super::super::ir::Op;

/// 从 `Not` 的操作数算起的最大内联层数；再深的单用节点仍单独发变量。
const MAX_INLINE_DEPTH: usize = 3;

/// 标记哪些节点不单独发语句：`Not` 的操作数单用时内联，并沿它的单用操作数继续下钻。
pub(super) fn targets(ops: &[Op]) -> Vec<bool> {
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
    for op in ops {
        let Op::Not(inner) = *op else { continue };
        mark(&uses, &mut inlined, ops, inner, 1);
    }
    inlined
}

fn mark(uses: &[usize], inlined: &mut [bool], ops: &[Op], index: usize, depth: usize) {
    if depth > MAX_INLINE_DEPTH || uses[index] != 1 || inlined[index] {
        return;
    }
    inlined[index] = true;
    match ops[index] {
        Op::Const(_) | Op::Var(_) => {}
        Op::Not(inner) => mark(uses, inlined, ops, inner, depth + 1),
        Op::And(left, right)
        | Op::Or(left, right)
        | Op::Xor(left, right)
        | Op::Xor2(left, right)
        | Op::Implies(left, right)
        | Op::Equiv(left, right) => {
            mark(uses, inlined, ops, left, depth + 1);
            mark(uses, inlined, ops, right, depth + 1);
        }
    }
}

/// 操作数的 C 表达式：已内联的递归展开，未内联的取已经发射的 `tN`。
pub(super) fn expr(ops: &[Op], names: &[Option<usize>], inlined: &[bool], index: usize) -> String {
    if !inlined[index] {
        return format!("t{}", names[index].expect("未内联的操作数先于父节点发射"));
    }
    match ops[index] {
        Op::Const(value) => code(value).to_string(),
        Op::Var(slot) => format!("vars[{slot}]"),
        Op::And(left, right) => {
            format!(
                "trit_bin(T_AND, {}, {})",
                expr(ops, names, inlined, left),
                expr(ops, names, inlined, right)
            )
        }
        Op::Or(left, right) => {
            format!(
                "trit_bin(T_OR, {}, {})",
                expr(ops, names, inlined, left),
                expr(ops, names, inlined, right)
            )
        }
        Op::Xor(left, right) => {
            format!(
                "trit_bin(T_XOR, {}, {})",
                expr(ops, names, inlined, left),
                expr(ops, names, inlined, right)
            )
        }
        Op::Xor2(left, right) => {
            format!(
                "trit_bin(T_XOR2, {}, {})",
                expr(ops, names, inlined, left),
                expr(ops, names, inlined, right)
            )
        }
        Op::Implies(left, right) => {
            format!(
                "trit_bin(T_IMPL, {}, {})",
                expr(ops, names, inlined, left),
                expr(ops, names, inlined, right)
            )
        }
        Op::Equiv(left, right) => {
            format!(
                "trit_bin(T_EQUIV, {}, {})",
                expr(ops, names, inlined, left),
                expr(ops, names, inlined, right)
            )
        }
        Op::Not(inner) => format!("trit_not({})", expr(ops, names, inlined, inner)),
    }
}

#[cfg(test)]
mod tests;
