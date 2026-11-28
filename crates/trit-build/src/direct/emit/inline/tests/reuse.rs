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
    assert_eq!(five.matches("C t0=").count(), 0, "{five}");
    assert_eq!(five.matches("v[0]").count(), 2, "{five}");
    assert!(five.contains("4*-v[0]"), "{five}");
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
    assert!(six.contains("C t0=v[0];"), "{six}");
    assert_eq!(six.matches("v[0]").count(), 1, "{six}");
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
    assert_eq!(source.matches("C t0=").count(), 0, "{source}");
    assert!(source.contains("?0:T[3*"), "{source}");
}
