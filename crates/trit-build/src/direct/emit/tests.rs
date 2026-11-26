//! 渲染内联判据：单用节点并成一条语句，多用子式仍单独落变量。

use super::render;
use crate::direct::ir::Op;
use trit_core::Algebra;

fn vars() -> Vec<String> {
    vec!["a".to_string(), "b".to_string()]
}

#[test]
fn single_use_and_is_inlined_into_not() {
    let source = render(
        vec![Op::Var(0), Op::Var(1), Op::And(0, 1), Op::Not(2)],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(
        source.matches("    int8_t t").count(),
        1,
        "单用锥只该剩根语句：{source}"
    );
    assert!(
        source.contains("int8_t t0 = trit_not(trit_bin(T_AND, vars[0], vars[1]));"),
        "{source}"
    );
    assert!(source.contains("return t0;"), "{source}");
    // 被内联的与式和输入都不该单独成句。
    assert!(!source.contains("int8_t t1 ="), "{source}");
}

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
    assert_eq!(source.matches("    int8_t t").count(), 2, "{source}");
    assert!(
        source.contains("int8_t t0 = trit_bin(T_AND, vars[0], vars[1]);"),
        "{source}"
    );
    assert!(
        source.contains("int8_t t1 = trit_bin(T_AND, trit_not(t0), t0);"),
        "{source}"
    );
}

#[test]
fn or_and_xor2_are_inlined_with_their_own_tables() {
    let or_source = render(
        vec![Op::Var(0), Op::Var(1), Op::Or(0, 1), Op::Not(2)],
        &vars(),
        Algebra::Kleene,
    );
    assert!(
        or_source.contains("trit_not(trit_bin(T_OR, vars[0], vars[1]))"),
        "{or_source}"
    );
    let xor_source = render(
        vec![Op::Var(0), Op::Var(1), Op::Xor2(0, 1), Op::Not(2)],
        &vars(),
        Algebra::Kleene,
    );
    assert!(
        xor_source.contains("trit_not(trit_bin(T_XOR2, vars[0], vars[1]))"),
        "{xor_source}"
    );
}
