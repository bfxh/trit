//! 测试用的 IR 解释器：按 `Algebra` 逐条执行 `Op`，用于与 `trit_eval` 对拍。

use trit_core::Algebra;
use trit_core::value::Ternary;

use crate::direct::ir::Op;

pub(super) fn eval_ops(ops: &[Op], values: &[Ternary], alg: Algebra) -> Ternary {
    let mut registers: Vec<Ternary> = Vec::with_capacity(ops.len());
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
