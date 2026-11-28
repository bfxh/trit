//! `direct.c` 函数体：临时变量合并成一条声明，根节点直接返回。

use super::super::ir::Op;
use super::inline;

mod value;

pub(super) fn body(
    ops: &[Op],
    vars: &[String],
    inlined: &[bool],
    tables: &inline::Tables,
) -> String {
    let mut text = String::from("C f(C*v,int n){");
    let guard = if vars.is_empty() {
        None
    } else {
        Some(format!("n<{}?0:", vars.len()))
    };
    let mut names: Vec<Option<usize>> = vec![None; ops.len()];
    let mut next = 0usize;
    for (index, op) in ops.iter().enumerate() {
        if inlined[index] {
            continue;
        }
        names[index] = Some(next);
        if next == 0 {
            text.push_str("C t0=");
        } else {
            text.push_str(&format!(",t{next}="));
        }
        text.push_str(&value::value(op, ops, &names, inlined, tables));
        next += 1;
    }
    if next > 0 {
        text.push(';');
    }
    text.push_str("return ");
    if let Some(guard) = &guard {
        text.push_str(guard);
    }
    let root = inline::expr(ops, &names, inlined, ops.len() - 1, tables);
    text.push_str(&root);
    text.push_str(";}");
    text
}
