//! 常量驱动的恒等式化简：单位律、对合律、幂等律三代数通用；吸收律与格吸收律按代数分派。
//! `x ∧ T = x`、`x ∨ F = x`、`¬¬x = x`、`x ∧ x = x`、`x ∨ x = x` 三种代数都成立；
//! K3/Ł3 额外 `x ∧ F = F`、`x ∨ T = T`、`x ∧ (x ∨ y) = x`；B3 的 U 传染保留原式。
use super::super::ir::Op;
use super::super::lower::prune_dead;
use build::{push_binary, push_const, push_node};
use trit_core::Algebra;
use trit_core::value::Ternary;
mod build;
mod lattice;
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
            Op::Not(inner) => {
                let inner = resolve[inner];
                if let Op::Not(target) = kept[inner] {
                    resolve.push(target);
                } else {
                    resolve.push(kept.len());
                    kept.push(Op::Not(inner));
                    consts.push(None);
                }
            }
            Op::And(left, right) => {
                let (left, right) = (resolve[left], resolve[right]);
                if !matches!(alg, Algebra::Bochvar)
                    && (consts[left] == Some(Ternary::False)
                        || consts[right] == Some(Ternary::False))
                {
                    push_const(&mut resolve, &mut kept, &mut consts, Ternary::False);
                } else if let Some(alias) = lattice::absorbed(alg, &kept, left, right, true) {
                    resolve.push(alias);
                } else if left == right || consts[left] == Some(Ternary::True) {
                    resolve.push(right);
                } else if consts[right] == Some(Ternary::True) {
                    resolve.push(left);
                } else {
                    push_node(&mut resolve, &mut kept, &mut consts, Op::And(left, right));
                }
            }
            Op::Or(left, right) => {
                let (left, right) = (resolve[left], resolve[right]);
                if !matches!(alg, Algebra::Bochvar)
                    && (consts[left] == Some(Ternary::True) || consts[right] == Some(Ternary::True))
                {
                    push_const(&mut resolve, &mut kept, &mut consts, Ternary::True);
                } else if let Some(alias) = lattice::absorbed(alg, &kept, left, right, false) {
                    resolve.push(alias);
                } else if left == right || consts[left] == Some(Ternary::False) {
                    resolve.push(right);
                } else if consts[right] == Some(Ternary::False) {
                    resolve.push(left);
                } else {
                    push_node(&mut resolve, &mut kept, &mut consts, Op::Or(left, right));
                }
            }
            Op::Xor(left, right) => {
                push_binary(&mut resolve, &mut kept, &mut consts, left, right, Op::Xor)
            }
            Op::Xor2(left, right) => {
                push_binary(&mut resolve, &mut kept, &mut consts, left, right, Op::Xor2)
            }
            Op::Implies(left, right) => push_binary(
                &mut resolve,
                &mut kept,
                &mut consts,
                left,
                right,
                Op::Implies,
            ),
            Op::Equiv(left, right) => {
                push_binary(&mut resolve, &mut kept, &mut consts, left, right, Op::Equiv)
            }
        }
    }

    let root = *resolve.last().expect("非空 IR");
    prune_dead(kept, root)
}

#[cfg(test)]
mod tests;
