//! 叶子复用边界：≤5 次直写，≥6 次保留读取；常量总是直写。

use super::*;

#[test]
fn variable_reuse_boundary() {
    let five = render(
        vec![
            Op::Var(0),
            Op::And(0, 0),
            Op::And(0, 0),
            Op::Not(0),
            Op::Or(1, 2),
            Op::Or(4, 3),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(five.matches("    int8_t t").count(), 0, "{five}");
    assert_eq!(five.matches("vars[0]").count(), 5, "{five}");
    let six = render(
        vec![
            Op::Var(0),
            Op::And(0, 0),
            Op::And(0, 0),
            Op::And(0, 0),
            Op::Or(1, 2),
            Op::Or(4, 3),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert!(six.contains("int8_t t0 = vars[0];"), "{six}");
    assert_eq!(six.matches("vars[0]").count(), 1, "{six}");
}

#[test]
fn reused_constant_stays_inline() {
    let source = render(
        vec![
            Op::Const(trit_core::value::Ternary::True),
            Op::And(0, 0),
            Op::And(0, 0),
            Op::And(0, 0),
            Op::Or(1, 2),
            Op::Or(4, 3),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("    int8_t t").count(), 0, "{source}");
    assert!(source.contains("return T_OR[3*"), "{source}");
}
