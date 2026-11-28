//! 多临时变量的声明合并：两个扇出节点仍只落一条声明。

use super::super::render;
use crate::direct::ir::Op;
use trit_core::Algebra;

#[test]
fn many_calls_stay_inline() {
    let mut ops = vec![Op::Var(0), Op::Var(1), Op::And(0, 1)];
    for index in 2..22 {
        ops.push(Op::And(index, 1));
    }
    let source = render(ops, &["a".to_string(), "b".to_string()], Algebra::Kleene);
    assert!(!source.contains("trit_bin("), "{source}");
    assert!(source.contains("?0:T[3*T["), "{source}");
    assert_eq!(source.matches("T[3*").count(), 21, "{source}");
}

#[test]
fn multiple_temporaries_share_one_declaration() {
    let source = render(
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::And(0, 1),
            Op::Var(2),
            Op::Var(3),
            Op::And(2, 3),
            Op::Or(2, 5),
            Op::Xor(2, 5),
            Op::And(6, 7),
        ],
        &[
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ],
        Algebra::Kleene,
    );
    assert!(source.contains("C t0="), "{source}");
    assert!(source.contains(",t1="), "{source}");
    assert_eq!(
        source.matches(';').count(),
        4,
        "只允许 typedef、表、临时声明与 return 各一个分号：{source}"
    );
    assert!(!source.contains("; C t1="), "{source}");
}
