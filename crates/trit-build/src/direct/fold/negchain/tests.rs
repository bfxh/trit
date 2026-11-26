//! 德摩根链归一判据：三代数 3×3×3 逐格、正/对偶形状、非全取反保留、渲染面。

use super::collapse;
use crate::direct::fold::fold;
use crate::direct::ir::Op;
use trit_core::value::Ternary;
use trit_core::{Algebra, Parser};
use trit_eval::eval::{Assignment, eval};

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

/// 德摩根在三代数、三值全组合上成立；同一公式经 `fold` 后逐赋值仍与原式一致。
#[test]
fn chain_matches_eval_for_every_algebra_and_assignment() {
    for source in ["!(!a & !b & !c)", "!(!a | !b | !c)"] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let before = crate::direct::lower::lower(&expr, &vars).expect("降级");
        let count = 3u64.pow(vars.len() as u32);
        for alg in ALGS {
            let folded = fold(before.clone(), alg);
            assert!(folded.len() <= before.len(), "{alg:?} {source}");
            for code in 0..count {
                let mut digits = Vec::new();
                let mut assignment = Assignment::new();
                let mut rest = code;
                for var in &vars {
                    let value = VALUES[(rest % 3) as usize];
                    rest /= 3;
                    digits.push(value);
                    assignment.set(var.clone(), value);
                }
                assert_eq!(
                    run(&folded, &digits, alg),
                    eval(&expr, &assignment, alg),
                    "{alg:?} {source} code={code}"
                );
            }
        }
    }
}

#[test]
fn triple_negation_chain_collapses_to_dual() {
    let out = collapse(vec![
        Op::Var(0),
        Op::Not(0),
        Op::Var(1),
        Op::Not(2),
        Op::And(1, 3),
        Op::Var(2),
        Op::Not(5),
        Op::And(4, 6),
        Op::Not(7),
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
fn triple_negation_chain_dual_collapses_to_and() {
    let out = collapse(vec![
        Op::Var(0),
        Op::Not(0),
        Op::Var(1),
        Op::Not(2),
        Op::Or(1, 3),
        Op::Var(2),
        Op::Not(5),
        Op::Or(4, 6),
        Op::Not(7),
    ]);
    assert_eq!(
        out,
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Var(2),
            Op::And(0, 1),
            Op::And(3, 2)
        ]
    );
}

#[test]
fn mixed_operand_stays() {
    let ops = vec![
        Op::Var(0),
        Op::Not(0),
        Op::Var(1),
        Op::Not(2),
        Op::Var(2),
        Op::And(1, 3),
        Op::And(5, 4),
        Op::Not(6),
    ];
    assert_eq!(collapse(ops.clone()), ops);
}

fn run(ops: &[Op], values: &[Ternary], alg: Algebra) -> Ternary {
    let mut registers = Vec::with_capacity(ops.len());
    for op in ops {
        let value = match op {
            Op::Const(value) => *value,
            Op::Var(index) => values[*index],
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
