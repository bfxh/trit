//! 渲染内联判据：单用节点并进根返回，多用子式仍单独落变量。
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
        source.matches("int t").count(),
        0,
        "单用锥与根节点都直接展开，不留临时语句：{source}"
    );
    assert!(source.contains("?0:-T[3*v[0]+v[1]+4];"), "{source}");
    assert!(!source.contains("trit_bin("), "{source}");
    assert!(
        !source.contains("return t0;") && !source.contains("int t0="),
        "{source}"
    );
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
    assert_eq!(source.matches("int t").count(), 1, "{source}");
    assert!(source.contains("int t0=T["), "{source}");
    assert!(source.contains("?0:T[3*-t0"), "{source}");
    assert!(!source.contains("trit_bin("), "{source}");
    assert!(!source.contains(",t1="), "{source}");
}
#[test]
fn or_and_xor2_are_inlined_with_their_own_tables() {
    let or_source = render(
        vec![Op::Var(0), Op::Var(1), Op::Or(0, 1), Op::Not(2)],
        &vars(),
        Algebra::Kleene,
    );
    assert!(or_source.contains("-T[3*v[0]+v[1]+4]"), "{or_source}");
    assert!(!or_source.contains("trit_bin("), "{or_source}");
    let xor_source = render(
        vec![Op::Var(0), Op::Var(1), Op::Xor2(0, 1), Op::Not(2)],
        &vars(),
        Algebra::Kleene,
    );
    assert!(xor_source.contains("-T[3*v[0]+v[1]+4]"), "{xor_source}");
    assert!(!xor_source.contains("trit_bin("), "{xor_source}");
}
mod many;
