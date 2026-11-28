//! 直算渲染形状与真值表精确值。

use super::*;

/// 渲染出来的形状：函数签名/无 3^n 表/根节点直接返回。
#[test]
fn render_is_linear_not_a_truth_table() {
    let expr = Parser::parse("a & b | !c").expect("解析");
    let vars = expr.variables();
    let source = emit(&expr, &vars, Algebra::Kleene).expect("生成");
    assert!(source.starts_with("/*K3*/"), "{source}");
    assert!(source.contains("C f(C*v,int n){"), "{source}");
    assert!(source.contains("n<3?0:"), "{source}");
    assert!(source.contains("C T[]"), "{source}");
    assert!(source.contains("-v["), "{source}");
    assert!(!source.contains("T_NOT"), "{source}");
    assert!(
        !source.contains("TRIT_TABLE"),
        "直算后端不该有 3^n 表：{source}"
    );
    assert!(source.contains("?0:T["), "{source}");
}

#[test]
fn binary_and_table_is_exact() {
    let expr = Parser::parse("a & b").expect("解析");
    let source = emit(&expr, &expr.variables(), Algebra::Kleene).expect("生成");
    assert!(source.contains("T[]={-1,-1,-1,-1,0,0,-1,0,1};"), "{source}");
}
