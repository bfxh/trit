//! K3 `⇒` 在 `absorb::or` 前不能把 `kept` 下标当原始 IR 下标二次解析。

use crate::direct::emit;
use trit_core::{Algebra, Parser};

#[test]
fn k3_implication_reuses_or_without_forward_resolve_index() {
    let expr = Parser::parse("(((b => a) | (a | b)) ^ (a => b))").expect("解析");
    let c = emit(&expr, &expr.variables(), Algebra::Kleene).expect("生成");
    assert!(c.contains("C f(C*v,int n){"), "{c}");
}
