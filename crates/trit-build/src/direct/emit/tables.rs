//! 收集本次渲染用到的二元表，顺序固定；单表保留原名，多表交给 `inline` 合并。

use super::super::ir::Op;
use super::super::{binary_table, gate_table};
use trit_circuit::GateKind;
use trit_core::Algebra;

pub(super) fn used(ops: &[Op], alg: Algebra) -> Vec<(&'static str, [i8; 9])> {
    let mut has_and = false;
    let mut has_or = false;
    let mut has_xor = false;
    let mut has_xor2 = false;
    let mut has_implies = false;
    let mut has_equiv = false;
    for op in ops {
        match op {
            Op::And(_, _) => has_and = true,
            Op::Or(_, _) => has_or = true,
            Op::Xor(_, _) => has_xor = true,
            Op::Xor2(_, _) => has_xor2 = true,
            Op::Implies(_, _) => has_implies = true,
            Op::Equiv(_, _) => has_equiv = true,
            Op::Not(_) | Op::Const(_) | Op::Var(_) => {}
        }
    }
    let mut tables = Vec::new();
    if has_and {
        tables.push(("T_AND", binary_table(alg, Algebra::and)));
    }
    if has_or {
        tables.push(("T_OR", binary_table(alg, Algebra::or)));
    }
    if has_xor {
        tables.push(("T_XOR", binary_table(alg, Algebra::strong_add)));
    }
    if has_xor2 {
        tables.push(("T_XOR2", gate_table(alg, GateKind::XorClassic)));
    }
    if has_implies {
        tables.push(("T_IMPL", binary_table(alg, Algebra::implies)));
    }
    if has_equiv {
        tables.push(("T_EQUIV", binary_table(alg, Algebra::equiv)));
    }
    tables
}
