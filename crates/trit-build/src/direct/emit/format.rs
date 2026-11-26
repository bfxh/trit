//! `direct.c` 的固定片段：运算符表与代数名。

use trit_core::Algebra;

mod table;

pub(super) use table::push_table;

pub(super) fn neg(inner: String) -> String {
    if inner.starts_with('-') {
        format!("-({inner})")
    } else {
        format!("-{inner}")
    }
}

pub(super) fn alg_name(alg: Algebra) -> &'static str {
    match alg {
        Algebra::Kleene => "kleene",
        Algebra::Lukasiewicz => "lukasiewicz",
        Algebra::Bochvar => "bochvar",
    }
}
