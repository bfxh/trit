//! 已解析成 `kept` 下标的取反；给 `kept` 上已经工作的连接词复用。

use super::super::build::{push_const, push_node};
use super::de_morgan::de_morgan;
use crate::direct::ir::Op;
use trit_core::value::Ternary;

pub(crate) fn push_resolved(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    inner: usize,
) {
    if let Some(value) = consts[inner] {
        push_const(resolve, kept, consts, !value);
        return;
    }
    if let Op::Not(target) = kept[inner] {
        resolve.push(target);
        return;
    }
    if let Some(op) = de_morgan(kept, inner) {
        push_node(resolve, kept, consts, op);
        return;
    }
    push_node(resolve, kept, consts, Op::Not(inner));
}
