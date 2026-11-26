//! 常量驱动的恒等式化简：单位律、对合律、幂等律三代数通用；吸收律与格吸收律按代数分派。
//! `x ∧ T = x`、`x ∨ F = x`、`¬¬x = x`、`x ∧ x = x`、`x ∨ x = x` 三种代数都成立；
//! K3/Ł3 额外 `x ∧ F = F`、`x ∨ T = T`、`x ∧ (x ∨ y) = x`；B3 的 U 传染保留原式。
//! B3 另做补元消去 `x ∧ (¬x ∨ y) = x ∧ y`、`x ∨ (¬x ∧ y) = x ∨ y`，K3/Ł3 有 `U` 反例。
use super::super::ir::Op;
use super::super::lower::prune_dead;
use build::push_const;
use trit_core::Algebra;
use trit_core::value::Ternary;
mod absorb;
mod build;
mod complement;
mod connectives;
mod lattice;
mod negation;
mod xor;
mod xor2;
pub(super) fn simplify(ops: Vec<Op>, alg: Algebra) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let mut resolve: Vec<usize> = Vec::with_capacity(ops.len());
    let mut kept: Vec<Op> = Vec::with_capacity(ops.len());
    let mut consts: Vec<Option<Ternary>> = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            Op::Const(value) => push_const(&mut resolve, &mut kept, &mut consts, value),
            Op::Var(slot) => {
                resolve.push(kept.len());
                kept.push(Op::Var(slot));
                consts.push(None);
            }
            Op::Not(inner) => negation::push(&mut resolve, &mut kept, &mut consts, inner),
            Op::And(left, right) => {
                absorb::and(&mut resolve, &mut kept, &mut consts, left, right, alg);
            }
            Op::Or(left, right) => {
                absorb::or(&mut resolve, &mut kept, &mut consts, left, right, alg);
            }
            Op::Xor(left, right) => {
                xor::push(&mut resolve, &mut kept, &mut consts, left, right, alg)
            }
            Op::Xor2(left, right) => xor2::push(&mut resolve, &mut kept, &mut consts, left, right),
            Op::Implies(left, right) => {
                connectives::implies(&mut resolve, &mut kept, &mut consts, left, right, alg)
            }
            Op::Equiv(left, right) => {
                connectives::equiv(&mut resolve, &mut kept, &mut consts, left, right, alg)
            }
        }
    }
    let root = *resolve.last().expect("非空 IR");
    prune_dead(kept, root)
}

#[cfg(test)]
mod tests;
