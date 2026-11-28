//! 二元表的 C 索引：普通表直接 `3*l+r`；`And` 借 `Or` 表时按德摩根改写。
use crate::direct::emit::{format, inline::Table};
pub(crate) fn call(table: Table, left: String, right: String) -> String {
    let (left, right) = if table.negated {
        (format::neg(left), format::neg(right))
    } else {
        (left, right)
    };
    let body = if left == right {
        format!("T[4*{left}+{}]", table.offset)
    } else {
        let sign = if right.starts_with('-') { "" } else { "+" };
        format!("T[3*{left}{sign}{right}+{}]", table.offset)
    };
    if table.negated {
        return format::neg(body);
    }
    body
}
