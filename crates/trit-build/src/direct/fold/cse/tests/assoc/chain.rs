//! 结合链的 `Or` 对偶与渲染面判据。

use super::super::super::cse;
use crate::direct::emit;
use crate::direct::fold::fold;
use crate::direct::ir::Op;
use crate::direct::lower::lower;
use trit_core::{Algebra, Parser};

#[test]
fn mirrored_or_associations_share_one_chain() {
    let out = cse(vec![
        Op::Var(0),
        Op::Var(1),
        Op::Var(2),
        Op::Or(0, 1),
        Op::Or(3, 2),
        Op::Or(1, 2),
        Op::Or(0, 5),
        Op::And(4, 6),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Var(2),
            Op::Or(0, 1),
            Op::Or(3, 2)
        ]
    );
}

#[test]
fn render_shares_the_mirrored_and_chain() {
    let expr = Parser::parse("((a & c) & b) | (a & (b & c))").expect("解析");
    let vars = expr.variables();
    let before = lower(&expr, &vars).expect("降级");
    assert_eq!(before.len(), 8, "{before:?}");
    let after = fold(before, Algebra::Kleene);
    assert_eq!(after.len(), 5, "{after:?}");
    let c = emit(&expr, &vars, Algebra::Kleene).expect("生成");
    assert_eq!(c.matches("    int8_t t").count(), 0, "{c}");
    assert!(!c.contains("T_OR"), "{c}");
    assert!(c.contains("return trit_bin(T_AND"), "{c}");
}
