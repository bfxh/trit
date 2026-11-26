//! 常量折叠判据：折叠后的 IR 仍与 `trit_eval` 全赋值一致；全常量树塌成一条
//! `Const`，渲染时不再拉算子表。

use super::*;
use crate::direct::emit;
use trit_core::Parser;
use trit_eval::eval::{Assignment, eval};

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];
mod factor;
/// `Xor2` 来自 Verilog 网表，必须按 `GateKind::XorClassic` 的表折，不能走 `alg.strong_add`。
#[test]
fn classic_xor2_constants_fold_like_the_gate_table() {
    for alg in ALGS {
        for left in VALUES {
            for right in VALUES {
                let ops = vec![Op::Const(left), Op::Const(right), Op::Xor2(0, 1)];
                let expected = GateKind::XorClassic.eval(&[left, right], alg);
                assert_eq!(
                    fold(ops, alg),
                    vec![Op::Const(expected)],
                    "{alg:?} {left:?} ^ {right:?}"
                );
            }
        }
    }
}

/// 全常量表达式：三种代数下都必须折成恰好一条 `Const`。
#[test]
fn all_constant_tree_collapses_to_one_const() {
    let expr = Parser::parse("((1 & X) | !0) <=> (X => 0)").expect("解析");
    let vars = expr.variables();
    assert!(vars.is_empty());
    for alg in ALGS {
        let ops = fold(
            crate::direct::lower::lower(&expr, &vars).expect("降级"),
            alg,
        );
        assert_eq!(ops.len(), 1, "{alg:?}: {ops:?}");
        let expected = eval(&expr, &Assignment::new(), alg);
        assert_eq!(ops, vec![Op::Const(expected)], "{alg:?}");
    }
}

/// 混合树里只折全常量节点，折叠前后逐赋值等价（3^n 穷举，三种代数）。
#[test]
fn folded_ir_matches_eval_for_every_assignment() {
    let sources = [
        "(1 & 0) & a",
        "a | (X ^ 1)",
        "((1 & X) | 0) <=> b",
        "1 & 0 | !X",
        "a & b | !c",
    ];
    for source in sources {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let ops = crate::direct::lower::lower(&expr, &vars).expect("降级");
        let count = 3u64.pow(vars.len() as u32);
        for alg in ALGS {
            let folded = fold(ops.clone(), alg);
            assert!(folded.len() <= ops.len(), "{alg:?} {source}");
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

/// 全常量树渲染成一条语句，不注册 `T_NOT` / `T_AND` 等表。
#[test]
fn constant_c_has_one_statement_and_no_operator_tables() {
    let expr = Parser::parse("(1 & X) | !0").expect("解析");
    let vars = expr.variables();
    for alg in ALGS {
        let source = emit(&expr, &vars, alg).expect("生成");
        assert_eq!(
            source.matches("    int8_t t").count(),
            1,
            "{alg:?}: {source}"
        );
        assert!(
            !source.contains("static const int8_t T_"),
            "{alg:?}: {source}"
        );
        assert!(!source.contains("trit_bin("), "{alg:?}: {source}");
        assert!(!source.contains("trit_not("), "{alg:?}: {source}");
        assert!(source.contains("return t0;"), "{alg:?}: {source}");
    }
}

/// 测试用解释器：只执行折叠后的 IR，作为「折叠是否保留语义」的被测对象。
fn run(ops: &[Op], values: &[Ternary], alg: Algebra) -> Ternary {
    let mut registers: Vec<Ternary> = Vec::with_capacity(ops.len());
    for op in ops {
        let value = match op {
            Op::Const(value) => *value,
            Op::Var(slot) => values[*slot],
            Op::Not(inner) => alg.not(registers[*inner]),
            Op::And(left, right) => alg.and(registers[*left], registers[*right]),
            Op::Or(left, right) => alg.or(registers[*left], registers[*right]),
            Op::Xor(left, right) => alg.strong_add(registers[*left], registers[*right]),
            Op::Xor2(left, right) => {
                let left = registers[*left];
                let right = registers[*right];
                if left.is_unknown() || right.is_unknown() {
                    Ternary::Unknown
                } else if left.is_true() != right.is_true() {
                    Ternary::True
                } else {
                    Ternary::False
                }
            }
            Op::Implies(left, right) => alg.implies(registers[*left], registers[*right]),
            Op::Equiv(left, right) => alg.equiv(registers[*left], registers[*right]),
        };
        registers.push(value);
    }
    *registers.last().expect("空 IR")
}
