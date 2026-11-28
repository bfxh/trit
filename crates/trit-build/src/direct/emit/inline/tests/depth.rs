//! 深链：单用节点不再有深度上限。

use super::*;
/// 四层单用 `And` 链整个并进根返回，不在第三层落临时变量。
#[test]
fn deep_single_use_chain_is_not_capped() {
    let source = render(
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::And(0, 1),
            Op::Var(2),
            Op::And(2, 3),
            Op::Var(3),
            Op::And(4, 5),
            Op::Const(trit_core::value::Ternary::False),
            Op::And(6, 7),
            Op::Not(8),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("C t0=").count(), 0, "{source}");
    assert!(source.contains("3*T[3*T[3*T["), "{source}");
}
