//! 单用节点的表达式展开：调度在 `plan`，这里只负责把表达式拼成 C。
//! 节点仍按后序发射；被标记为内联的节点不落临时变量，由父表达式递归展开。
//! 二元调用是走 `trit_bin` helper 还是直接展开表索引，按整篇字节数择优。

use super::super::code;
use super::super::ir::Op;

mod plan;

pub(super) use plan::targets;

/// 操作数的 C 表达式：已内联的递归展开，未内联的取已经发射的 `tN`。
pub(super) fn expr(
    ops: &[Op],
    names: &[Option<usize>],
    inlined: &[bool],
    index: usize,
    inline_tables: bool,
) -> String {
    if !inlined[index] {
        return format!("t{}", names[index].expect("未内联的操作数先于父节点发射"));
    }
    match ops[index] {
        Op::Const(value) => code(value).to_string(),
        Op::Var(slot) => format!("vars[{slot}]"),
        Op::And(left, right) => bin("T_AND", ops, names, inlined, left, right, inline_tables),
        Op::Or(left, right) => bin("T_OR", ops, names, inlined, left, right, inline_tables),
        Op::Xor(left, right) => bin("T_XOR", ops, names, inlined, left, right, inline_tables),
        Op::Xor2(left, right) => bin("T_XOR2", ops, names, inlined, left, right, inline_tables),
        Op::Implies(left, right) => bin("T_IMPL", ops, names, inlined, left, right, inline_tables),
        Op::Equiv(left, right) => bin("T_EQUIV", ops, names, inlined, left, right, inline_tables),
        Op::Not(inner) => format!("-({})", expr(ops, names, inlined, inner, inline_tables)),
    }
}
fn bin(
    table: &str,
    ops: &[Op],
    names: &[Option<usize>],
    inlined: &[bool],
    left: usize,
    right: usize,
    inline_tables: bool,
) -> String {
    let left = expr(ops, names, inlined, left, inline_tables);
    let right = expr(ops, names, inlined, right, inline_tables);
    call(table, left, right, inline_tables)
}
pub(super) fn call(table: &str, left: String, right: String, inline_tables: bool) -> String {
    if inline_tables {
        format!("{table}[({left} + 1) * 3 + ({right} + 1)]")
    } else {
        format!("trit_bin({table}, {left}, {right})")
    }
}
#[cfg(test)]
mod tests;
