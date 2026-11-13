//! pe 对象模型（接缝占位）：`build` / `call` 一律弃权。
//!
//! 实现见 `docs/YARA-EXPR/03-PE-MODEL.md` 与 `05-TASKS.md` 的 T20–T25。

use super::expr::Expr;
use super::modreg::Node;
use super::xval::Val;

/// 从完整文件字节构造 pe 对象树；占位阶段一律 `Unknown`。
pub(super) fn build(_data: &[u8]) -> Node {
    Node::Unknown
}

/// 函数调用；占位阶段一律 `Unknown`。
pub(super) fn call(_owner: &Node, _name: &str, _args: &[Expr], _vals: &[Val]) -> Val {
    Val::Unknown
}
