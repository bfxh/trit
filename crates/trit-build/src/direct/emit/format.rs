//! `direct.c` 的固定片段：运算符表与代数名。

use trit_core::Algebra;

pub(super) fn push_table(text: &mut String, name: &str, table: &[i8]) {
    text.push_str("static const int8_t ");
    text.push_str(name);
    text.push('[');
    text.push_str(&table.len().to_string());
    text.push_str("] = {");
    for value in table {
        text.push(' ');
        text.push_str(&value.to_string());
        text.push(',');
    }
    text.push_str(" };\n");
}

pub(super) fn alg_name(alg: Algebra) -> &'static str {
    match alg {
        Algebra::Kleene => "kleene",
        Algebra::Lukasiewicz => "lukasiewicz",
        Algebra::Bochvar => "bochvar",
    }
}
