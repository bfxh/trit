//! `direct.c` 的固定片段：运算符表与代数名。

use trit_core::Algebra;

mod table;

pub(super) use table::push_table;

pub(super) fn neg(inner: String) -> String {
    match inner.strip_prefix('-') {
        Some(rest) => rest.to_string(),
        None => format!("-{inner}"),
    }
}

pub(super) fn alg_tag(alg: Algebra) -> &'static str {
    match alg {
        Algebra::Kleene => "K3",
        Algebra::Lukasiewicz => "L3",
        Algebra::Bochvar => "B3",
    }
}
