//! CSE 判据：合并化简后重新出现的同构子式，并对合并后暴露的 `x ∧ x` / `x ∨ x` 走幂等律；语义与 `trit_eval` 全赋值对拍。

use super::cse;
use crate::direct::fold::fold;
use crate::direct::ir::Op;
use crate::direct::lower::lower;
use trit_core::{Algebra, Parser, value::Ternary};
use trit_eval::eval::{Assignment, eval};

mod assoc;
mod factor;
const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];

/// 合成 IR：两个同构的与式合并，外层 `Or(x, x)` 再按幂等律别名。
#[test]
fn duplicate_and_idempotent_nodes_collapse() {
    let ops = vec![
        Op::Var(0),
        Op::Var(1),
        Op::And(0, 1),
        Op::And(1, 0),
        Op::Or(2, 3),
    ];
    assert_eq!(cse(ops), vec![Op::Var(0), Op::Var(1), Op::And(0, 1)]);
}

/// 真实表达式：`((a & 1) & b) | (a & b)` 化简后只剩 3 条语句，
/// 且三种代数的全赋值结果都与 `trit_eval` 一致。
#[test]
fn unit_alias_then_cse_keeps_semantics() {
    let source = "((a & 1) & b) | (a & b)";
    let expr = Parser::parse(source).expect("解析");
    let vars = expr.variables();
    let count = 3u64.pow(vars.len() as u32);
    for alg in ALGS {
        let before = lower(&expr, &vars).expect("降级");
        assert_eq!(before.len(), 7, "{alg:?}");
        let after = fold(before.clone(), alg);
        assert_eq!(
            after,
            vec![Op::Var(0), Op::Var(1), Op::And(0, 1)],
            "{alg:?}"
        );
        for code in 0..count {
            let mut assignment = Assignment::new();
            let mut digits = Vec::new();
            let mut rest = code;
            for var in &vars {
                let value = VALUES[(rest % 3) as usize];
                rest /= 3;
                digits.push(value);
                assignment.set(var.clone(), value);
            }
            let expected = eval(&expr, &assignment, alg);
            assert_eq!(eval_ops(&before, &digits, alg), expected, "{alg:?} {code}");
            assert_eq!(eval_ops(&after, &digits, alg), expected, "{alg:?} {code}");
        }
    }
}
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

fn eval_ops(ops: &[Op], values: &[Ternary], alg: Algebra) -> Ternary {
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
