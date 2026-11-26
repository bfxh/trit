//! helper 与表索引按整篇字节数择优：调用足够多时仍保留 helper。

use super::super::render;
use crate::direct::ir::Op;
use trit_core::Algebra;

#[test]
fn many_calls_keep_the_shared_helper() {
    let mut ops = vec![Op::Var(0), Op::Var(1), Op::And(0, 1)];
    for index in 2..22 {
        ops.push(Op::And(index, 1));
    }
    let source = render(ops, &["a".to_string(), "b".to_string()], Algebra::Kleene);
    assert!(source.contains("static inline int8_t trit_bin"), "{source}");
    assert!(source.contains("return trit_bin("), "{source}");
    assert!(!source.contains("T_AND[("), "{source}");
}
