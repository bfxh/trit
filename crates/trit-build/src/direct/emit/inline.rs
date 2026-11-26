//! 单用节点的表达式展开：调度在 `plan`，这里只负责把表达式拼成 C。
//!
//! 节点仍按后序发射；被标记为内联的节点不落临时变量，由父表达式递归展开。

use super::super::code;
use super::super::ir::Op;

mod plan;

pub(super) use plan::targets;

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
