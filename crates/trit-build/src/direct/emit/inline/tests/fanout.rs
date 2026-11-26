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
    assert_eq!(source.matches("    int8_t t").count(), 1, "{source}");
    assert!(source.contains("int8_t t0 = T_AND["), "{source}");
    assert!(
        source.contains("return T_OR[(t0 + 1) * 3 + (t0 + 1)];"),
        "{source}"
    );
}
