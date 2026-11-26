//! 取反内联的下钻边界：单用才并、扇出即停、深度封顶。

use super::super::render;
use crate::direct::ir::Op;
use trit_core::Algebra;

fn vars() -> Vec<String> {
    vec!["a".to_string(), "b".to_string(), "c".to_string()]
}

/// `!a` 的变量只被取反这一处引用：取变量与取反并成一条。
#[test]
fn negated_single_use_variable_is_inlined() {
    let source = render(vec![Op::Var(0), Op::Not(0)], &vars(), Algebra::Kleene);
    assert_eq!(source.matches("    int8_t t").count(), 1, "{source}");
    assert!(
        source.contains("int8_t t0 = trit_not(vars[0]);"),
        "{source}"
    );
}

/// 单用锥里的节点若在别处还有引用，下钻在该节点停住，不复制扇出。
#[test]
fn fanout_stops_the_descent() {
    let source = render(
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::And(0, 1),
            Op::Not(2),
            Op::And(3, 1),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("    int8_t t").count(), 3, "{source}");
    assert!(
        source.contains("trit_not(trit_bin(T_AND, vars[0], t0))"),
        "共享输入要引用已发射变量：{source}"
    );
}

/// 从 `Not` 的操作数算起超过 3 层就不再下钻，深层叶子仍是临时变量。
#[test]
fn depth_cap_keeps_deep_leaves_declared() {
    let source = render(
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::And(0, 1),
            Op::Var(2),
            Op::And(2, 3),
            Op::Var(3),
            Op::And(4, 5),
            Op::Not(6),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("    int8_t t").count(), 3, "{source}");
    assert!(source.contains("int8_t t0 = vars[0];"), "{source}");
    assert!(source.contains("int8_t t1 = vars[1];"), "{source}");
}

/// 三层以内的单用锥整个并进根语句：`Not(And(Or(a,b),c))` 只剩一条。
#[test]
fn multilevel_single_use_cone_fits_one_statement() {
    let source = render(
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Or(0, 1),
            Op::Var(2),
            Op::And(2, 3),
            Op::Not(4),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("    int8_t t").count(), 1, "{source}");
    assert!(
        source.contains("trit_not(trit_bin(T_AND, trit_bin(T_OR, vars[0], vars[1]), vars[2]))"),
        "{source}"
    );
}
