//! 扇出边界：共享节点不复制。

use super::*;

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
    assert_eq!(source.matches("    int8_t t").count(), 1, "{source}");
    assert!(source.contains("return T_AND[(-(T_AND["), "{source}");
}
