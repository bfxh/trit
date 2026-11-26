//! 单用节点的表达式展开：调度在 `plan`，表达式展开在 `expr`。
//! 节点仍按后序发射；被标记为内联的节点不落临时变量，沿内联锥并进父表达式。
//! 二元调用直接展开 `3*l+r+4` 表索引；紧凑索引比 helper 调用短，不再需要 helper。

mod expr;
mod plan;

pub(super) use expr::expr;
pub(super) use plan::targets;

pub(super) fn call(table: &str, left: String, right: String) -> String {
    format!("{table}[3*{left}+{right}+4]")
}
#[cfg(test)]
mod tests;
