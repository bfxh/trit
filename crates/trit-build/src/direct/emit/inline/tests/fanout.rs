//! 扇出边界：共享节点不复制。

use super::*;

/// 单用锥里的二元节点若在别处还有引用，下钻在该节点停住，单独发射一次。
#[test]
fn fanout_stops_the_descent() {
    let source = render(
        vec![Op::Var(0), Op::Var(1), Op::And(0, 1), Op::Or(2, 2)],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("    int t").count(), 1, "{source}");
    assert!(source.contains("int t0=T["), "{source}");
    assert!(source.contains("return T[3*t0+t0+13];"), "{source}");
}
