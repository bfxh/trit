//! 多次调用也不落 helper：紧凑表索引始终比 helper 调用短。

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
    assert!(source.contains("return T_AND[3*T_AND["), "{source}");
    assert_eq!(source.matches("T_AND[3*").count(), 21, "{source}");
}
