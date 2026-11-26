//! 深度封顶：越过 3 层的单用链重新找发射点。

use super::*;

/// 从发射父节点算起超过 3 层就不再下钻，边界节点自己发射；根节点仍直接返回。
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
    assert_eq!(source.matches("    int8_t t").count(), 2, "{source}");
    assert!(source.contains("int8_t t0 = vars[0];"), "{source}");
    assert!(source.contains("return -("), "{source}");
}
