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
    let mut text = String::new();
    text.push_str("int8_t trit_direct(const int8_t*v,unsigned n){\n");
    if !vars.is_empty() {
        text.push_str("    if(n<");
        text.push_str(&vars.len().to_string());
        text.push_str(")return 0;/*X*/\n");
    } else {
        text.push_str("    (void)n;\n");
    }
    let mut names: Vec<Option<usize>> = vec![None; ops.len()];
    let mut next = 0usize;
    for (index, op) in ops.iter().enumerate() {
        if inlined[index] {
            continue;
        }
        names[index] = Some(next);
        if next == 0 {
            text.push_str("    int t0=");
        } else {
            text.push_str(&format!(",t{next}="));
        }
        text.push_str(&value::value(op, ops, &names, inlined, tables));
        next += 1;
    }
    if next > 0 {
        text.push_str(";\n");
    }
    text.push_str("    return ");
    let root = inline::expr(ops, &names, inlined, ops.len() - 1, tables);
    text.push_str(&root);
    text.push_str(";\n}\n");
    text
}
