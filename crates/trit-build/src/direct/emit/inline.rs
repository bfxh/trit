//! 单用节点的表达式展开：调度在 `plan`，表达式展开在 `expr`。
//! 节点仍按后序发射；被标记为内联的节点不落临时变量，沿内联锥并进父表达式。
//! 二元调用直接展开 `3*l+r+4` 表索引；紧凑索引比 helper 调用短，不再需要 helper。
//! 多张表合并成一个 `T[]` 时，每个算子的表基址写进索引常量，引用处只保留 `T`。

mod expr;
mod plan;
mod table;

pub(super) use expr::expr;
pub(super) use plan::targets;
pub(crate) use table::{Table, Tables, call, tables};

#[cfg(test)]
mod tests;
