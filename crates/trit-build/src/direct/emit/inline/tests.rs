//! 单用内联的边界：单用才并、扇出即停、深度封顶。

use super::super::render;
use crate::direct::ir::Op;
use trit_core::Algebra;

fn vars() -> Vec<String> {
    vec!["a".to_string(), "b".to_string(), "c".to_string()]
}

/// `!a` 的变量与根取反都直接展开，一条临时语句也不留。
#[test]
fn negated_single_use_variable_is_inlined() {
    let source = render(vec![Op::Var(0), Op::Not(0)], &vars(), Algebra::Kleene);
    assert_eq!(source.matches("    int8_t t").count(), 0, "{source}");
    assert!(source.contains("return trit_not(vars[0]);"), "{source}");
}

mod depth;
mod fanout;
mod tree;
