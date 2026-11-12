//! ast 的单元测试（从 src/ast.rs 拆出，控制单文件行数预算）。

use super::*;
use crate::value::Ternary;

#[test]
fn collects_variables_in_order() {
    let e = Expr::and(
        Expr::Var("a".into()),
        Expr::or(Expr::Var("b".into()), Expr::Var("a".into())),
    );
    assert_eq!(e.variables(), vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn node_count_and_depth() {
    let e = Expr::and(Expr::Var("a".into()), Expr::Var("b".into()));
    assert_eq!(e.node_count(), 3);
    assert_eq!(e.depth(), 2);
}

#[test]
fn infix_rendering() {
    let e = Expr::logical_not(Expr::Var("a".into()));
    // F-04：Not 不再额外套括号（二元子式自带括号，故 `¬a`、`¬(a∧b)` 都可再解析）。
    assert_eq!(e.to_infix(), "¬a");
    let e2 = Expr::equiv(Expr::Const(Ternary::True), Expr::Var("x".into()));
    assert_eq!(e2.to_infix(), "(1 ≡ x)");
    // F-04 回归：128 层 `!` 渲染后必须能再解析（旧实现 `¬(...)` 把深度翻倍 ⇒ TooDeep）。
    let src = format!("{}a", "!".repeat(128));
    let deep = crate::Parser::parse(&src).expect("原表达式可解析");
    crate::Parser::parse(&deep.to_infix()).expect("渲染输出必须可再解析（F-04）");
}

#[test]
fn try_var_rejects_reserved_and_malformed_names() {
    assert_eq!(Expr::try_var("a_1").unwrap(), Expr::Var("a_1".into()));
    assert!(matches!(
        Expr::try_var("X").unwrap_err(),
        VarNameError::Reserved(_)
    ));
    assert!(matches!(
        Expr::try_var("a b").unwrap_err(),
        VarNameError::Invalid(_)
    ));
    assert!(matches!(
        Expr::try_var("").unwrap_err(),
        VarNameError::Empty
    ));
}

#[test]
fn display_does_not_round_trip_arbitrary_vars() {
    // 直接构造的 Var("X") 渲染成 X，重新解析会变成常量。
    let e = Expr::Var("X".into());
    assert_eq!(e.to_string(), "X");
    assert!(matches!(
        crate::parser::Parser::parse(&e.to_string()).unwrap(),
        Expr::Const(Ternary::Unknown)
    ));
}

#[test]
fn iterative_metrics_match_hand_computed_values() {
    let e = Expr::and(
        Expr::Var("a".into()),
        Expr::implies(Expr::Var("b".into()), Expr::Var("c".into())),
    );
    assert_eq!(e.node_count(), 5);
    assert_eq!(e.depth(), 3);
    assert_eq!(e.variables(), vec!["a", "b", "c"]);
}
