//! `direct.c` 函数体：同一棵 IR 分别按 helper 调用与表索引展开，由 `render` 选短者。

use super::super::code;
use super::super::ir::Op;
use super::inline;

pub(super) fn body(ops: &[Op], vars: &[String], inlined: &[bool], inline_tables: bool) -> String {
    let mut text = String::new();
    text.push_str("int8_t trit_direct(const int8_t *vars, unsigned nvars) {\n");
    if !vars.is_empty() {
        text.push_str("    if (nvars < ");
        text.push_str(&vars.len().to_string());
        text.push_str("u) return 0; /* 入参不足：按 X 处理，fail-closed */\n");
    } else {
        text.push_str("    (void)nvars;\n");
    }
    let mut names: Vec<Option<usize>> = vec![None; ops.len()];
    let mut next = 0usize;
    for (index, op) in ops.iter().enumerate() {
        if inlined[index] {
            continue;
        }
        names[index] = Some(next);
        let bin = |table: &str, left, right| {
            let left = inline::expr(ops, &names, inlined, left, inline_tables);
            let right = inline::expr(ops, &names, inlined, right, inline_tables);
            format!(
                "    int8_t t{next} = {};\n",
                inline::call(table, left, right, inline_tables)
            )
        };
        let line = match op {
            Op::Const(value) => format!("    int8_t t{next} = {};\n", code(*value)),
            Op::Var(slot) => format!("    int8_t t{next} = vars[{slot}];\n"),
            Op::Not(inner) => format!(
                "    int8_t t{next} = -({});\n",
                inline::expr(ops, &names, inlined, *inner, inline_tables)
            ),
            Op::And(left, right) => bin("T_AND", *left, *right),
            Op::Or(left, right) => bin("T_OR", *left, *right),
            Op::Xor(left, right) => bin("T_XOR", *left, *right),
            Op::Xor2(left, right) => bin("T_XOR2", *left, *right),
            Op::Implies(left, right) => bin("T_IMPL", *left, *right),
            Op::Equiv(left, right) => bin("T_EQUIV", *left, *right),
        };
        text.push_str(&line);
        next += 1;
    }
    text.push_str("    return ");
    let root = inline::expr(ops, &names, inlined, ops.len() - 1, inline_tables);
    text.push_str(&root);
    text.push_str(";\n}\n");
    text
}
