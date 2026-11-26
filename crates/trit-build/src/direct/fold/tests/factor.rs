//! 公因子提取：`(a ∧ b) ∨ (a ∧ c)` 与对偶式在三种代数下逐赋值等价，且节点变少。

use super::{ALGS, VALUES, run};
use crate::direct::fold::fold;
use trit_core::Parser;
use trit_eval::eval::{Assignment, eval};

#[test]
fn factoring_matches_eval_for_every_assignment() {
    for source in ["(a & b) | (a & c)", "(a | b) & (a | c)"] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let ops = crate::direct::lower::lower(&expr, &vars).expect("降级");
        assert_eq!(ops.len(), 6, "{source}");
        let count = 3u64.pow(vars.len() as u32);
        for alg in ALGS {
            let folded = fold(ops.clone(), alg);
            assert!(folded.len() < ops.len(), "{alg:?} {source}");
            for code_value in 0..count {
                let mut assignment = Assignment::new();
                let mut digits = Vec::new();
                let mut rest = code_value;
                for var in &vars {
                    let digit = rest % 3;
                    rest /= 3;
                    digits.push(VALUES[digit as usize]);
                    assignment.set(var.clone(), VALUES[digit as usize]);
                }
                assert_eq!(
                    run(&folded, &digits, alg),
                    eval(&expr, &assignment, alg),
                    "{alg:?} {source} code={code_value}"
                );
            }
        }
    }
}
