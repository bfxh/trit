//! 表达式展开的栈深测试。

use super::*;
use crate::direct::ir::Op;
use trit_core::value::Ternary;

/// 两万层单用链只走显式栈，不靠递归。
#[test]
fn deep_inline_chain_does_not_recurse() {
    let mut ops = vec![Op::Const(Ternary::False)];
    for _ in 0..20_000 {
        let last = ops.len() - 1;
        ops.push(Op::And(last, 0));
    }
    let inlined = vec![true; ops.len()];
    assert!(super::super::plan::targets(&ops).iter().all(|flag| *flag));
    let names = vec![None; ops.len()];
    let source = expr(
        &ops,
        &names,
        &inlined,
        ops.len() - 1,
        &Tables::named("T_AND"),
    );
    assert!(source.starts_with("T[") && source.ends_with(']'));
}
