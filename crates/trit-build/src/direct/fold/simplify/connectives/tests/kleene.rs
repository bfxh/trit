//! K3 蕴含降格：复用 `Or` 表，且不破坏化简的 `resolve` 一对一。

use crate::direct::emit;
use trit_core::{Algebra, Parser};

#[test]
fn k3_implication_shares_or_table_and_keeps_resolve_aligned() {
    let expr = Parser::parse("((a => b) <=> (!a | b)) ^ c").expect("解析");
    let c = emit(&expr, &expr.variables(), Algebra::Kleene).expect("生成");
    assert_eq!(c.len(), 231, "{c}");
    assert!(!c.contains("T_IMPL"), "{c}");
    assert!(c.contains("int t0=T[3*v[1]-v[0]+4];"), "{c}");
    assert!(c.contains("?0:T[3*T[4*t0+22]+v[2]+13];"), "{c}");
}
