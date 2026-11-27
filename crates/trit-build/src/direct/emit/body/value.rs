//! 单个非内联节点的右值：常量/变量叶子或展开后的二元调用。

use super::super::super::code;
use super::super::super::ir::Op;
use super::super::{format, inline};

pub(super) fn value(
    op: &Op,
    ops: &[Op],
    names: &[Option<usize>],
    inlined: &[bool],
    tables: &inline::Tables,
) -> String {
    let bin = |table, left, right| {
        let left = inline::expr(ops, names, inlined, left, tables);
        let right = inline::expr(ops, names, inlined, right, tables);
        inline::call(table, left, right)
    };
    match op {
        Op::Const(value) => code(*value).to_string(),
        Op::Var(slot) => format!("v[{slot}]"),
        Op::Not(inner) => format::neg(inline::expr(ops, names, inlined, *inner, tables)),
        Op::And(left, right) => bin(tables.and(), *left, *right),
        Op::Or(left, right) => bin(tables.or(), *left, *right),
        Op::Xor(left, right) => bin(tables.xor(), *left, *right),
        Op::Xor2(left, right) => bin(tables.xor2(), *left, *right),
        Op::Implies(left, right) => bin(tables.implies(), *left, *right),
        Op::Equiv(left, right) => bin(tables.equiv(), *left, *right),
    }
}
