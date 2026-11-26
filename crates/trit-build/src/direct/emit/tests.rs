//! 渲染内联判据：单用取反子式并成一条语句，多用子式仍单独落变量。

use super::render;
use crate::direct::ir::Op;
use trit_core::Algebra;

fn vars() -> Vec<String> {
    vec!["a".to_string(), "b".to_string()]
}

/// `Not(And)` 的子式只被这一处引用：两条语句并成一条，且不再声明中间变量。
#[test]
fn single_use_and_is_inlined_into_not() {
    let source = render(
        vec![Op::Var(0), Op::Var(1), Op::And(0, 1), Op::Not(2)],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(
        source.matches("    int8_t t").count(),
        3,
        "两个输入 + 一条融合语句：{source}"
    );
    assert!(
        source.contains("int8_t t2 = trit_not(trit_bin(T_AND, t0, t1));"),
        "{source}"
    );
    assert!(source.contains("return t2;"), "{source}");
    // 被内联的与式不该单独成句。
    assert!(!source.contains("int8_t t1 = trit_bin(T_AND"), "{source}");
}

/// 与式同时被取反和别处引用时不能内联，否则临时变量会被跳过。
#[test]
fn shared_and_stays_a_separate_statement() {
    let source = render(
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::And(0, 1),
            Op::Not(2),
            Op::And(3, 2),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("    int8_t t").count(), 5, "{source}");
    assert!(
        source.contains("int8_t t2 = trit_bin(T_AND, t0, t1);"),
        "{source}"
    );
    assert!(source.contains("int8_t t3 = trit_not(t2);"), "{source}");
}

/// `Or` / `Xor2` 的单用取反同样走融合路径，表名按算子取。
#[test]
fn or_and_xor2_are_inlined_with_their_own_tables() {
    let or_source = render(
        vec![Op::Var(0), Op::Var(1), Op::Or(0, 1), Op::Not(2)],
        &vars(),
        Algebra::Kleene,
    );
    assert!(
        or_source.contains("trit_not(trit_bin(T_OR, t0, t1))"),
        "{or_source}"
    );
    let xor_source = render(
        vec![Op::Var(0), Op::Var(1), Op::Xor2(0, 1), Op::Not(2)],
        &vars(),
        Algebra::Kleene,
    );
    assert!(
        xor_source.contains("trit_not(trit_bin(T_XOR2, t0, t1))"),
        "{xor_source}"
    );
}
