//! 回归：饱和合并多个 e-class 后，提取不能把根退化成常量 `X`。
//!
//! 触发形状：`union` 后 `classes` 里仍留着指向被并掉 class 的旧子 id；提取的代价表
//! 只按根 id 建键，旧 id 查不到代价 ⇒ 整棵根退化成 `X` ⇒ CLI 把「重写不等价」当内部错误。

use super::*;
use crate::bitplan;
use trit_core::Parser;

const ALGEBRAS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];

#[test]
fn equiv_xor_chain_does_not_degrade_to_unknown() {
    let source = "((a => b) <=> (!a | b)) ^ c";
    let expr = Parser::parse(source).unwrap();
    let vars = expr.variables();
    let (rewritten, fired) = rewrite(&expr, Algebra::Kleene, 64);
    assert_ne!(
        rewritten.to_infix(),
        "X",
        "提取退化成了常量 X（fired={fired:?}）"
    );
    assert_eq!(
        bitplan::table(&rewritten, &vars, Algebra::Kleene, 1).unwrap(),
        bitplan::table(&expr, &vars, Algebra::Kleene, 1).unwrap(),
        "{source} 重写后语义变了：→ {}",
        rewritten.to_infix()
    );
}

#[test]
fn equiv_forms_keep_semantics_in_every_algebra() {
    for source in [
        "((a => b) <=> (!a | b)) ^ c",
        "(a <=> b) <=> (b <=> a)",
        "((a & b) <=> (b & a)) | c",
        "((a => b) & (b => a)) <=> (a <=> b)",
    ] {
        let expr = Parser::parse(source).unwrap();
        let vars = expr.variables();
        for alg in ALGEBRAS {
            let (rewritten, fired) = rewrite(&expr, alg, 64);
            assert_ne!(
                rewritten.to_infix(),
                "X",
                "{source} @ {alg:?} 提取退化成了常量 X（fired={fired:?}）"
            );
            assert_eq!(
                bitplan::table(&rewritten, &vars, alg, 1).unwrap(),
                bitplan::table(&expr, &vars, alg, 1).unwrap(),
                "{source} @ {alg:?} 重写后语义变了：→ {}",
                rewritten.to_infix()
            );
        }
    }
}
