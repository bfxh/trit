//! `direct.c` 的渲染：表由 `Algebra` 现算，节点按后序发语句。
//!
//! 单用节点在 `inline` 里决定是否并进父表达式（沿单用边展开，扇出即停），根节点由
//! `return` 直接展开。二元调用一律展开成 `T[3*l+r+基址]` 表索引：紧凑索引每次调用
//! 都比 `trit_bin` helper 调用短，helper 的固定声明无法摊销。

use trit_core::Algebra;

use super::ir::Op;
use format::{alg_tag, push_table};

mod body;
mod format;
mod inline;
mod tables;

/// 表达式 → `direct.c` 全文。
pub(super) fn render(ops: Vec<Op>, vars: &[String], alg: Algebra) -> String {
    let binary_tables = tables::used(&ops, alg);
    let inlined = inline::targets(&ops);
    let mut prefix = String::new();
    prefix.push_str("/*");
    prefix.push_str(alg_tag(alg));
    if !vars.is_empty() {
        prefix.push(' ');
        prefix.push_str(&vars.join(","));
    }
    prefix.push_str("*/");
    prefix.push_str("typedef signed char C;");
    let (tables, merged) = inline::tables(&binary_tables);
    if !merged.is_empty() {
        push_table(&mut prefix, "T", &merged);
    }

    let mut text = prefix;
    text.push_str(&body::body(&ops, vars, &inlined, &tables));
    text
}

#[cfg(test)]
mod tests;
