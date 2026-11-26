//! `simplify.rs` 的 IR 构造辅助：把「别名 / 常量 / 普通节点」写进 kept 与 consts。
//!
//! 拆出来只为行数预算：`simplify.rs` 的主循环已经接近上限，helper 不该挤在主线里。

use trit_core::value::Ternary;

use crate::direct::ir::Op;

pub(super) fn push_const(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    value: Ternary,
) {
    resolve.push(kept.len());
    kept.push(Op::Const(value));
    consts.push(Some(value));
}

pub(super) fn push_node(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    op: Op,
) {
    resolve.push(kept.len());
    kept.push(op);
    consts.push(None);
}
