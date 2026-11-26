//! 单用节点的表达式展开：调度在 `plan`，表达式展开在 `expr`。
//! 节点仍按后序发射；被标记为内联的节点不落临时变量，沿内联锥并进父表达式。
//! 二元调用是走 `trit_bin` helper 还是直接展开表索引，按整篇字节数择优。

mod expr;
mod plan;

pub(super) use expr::expr;
pub(super) use plan::targets;

pub(super) fn call(table: &str, left: String, right: String, inline_tables: bool) -> String {
    if inline_tables {
        format!("{table}[({left} + 1) * 3 + ({right} + 1)]")
    } else {
        format!("trit_bin({table}, {left}, {right})")
    }
}
#[cfg(test)]
mod tests;
