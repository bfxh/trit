//! 常量驱动的恒等式化简：只做三种代数都成立的格律。
//!
//! - `x ∧ T = x`、`x ∨ F = x`（单位律，结果别名到操作数）；
//! - `¬¬x = x`（对合律）。
//!
//! 不做 `x ∧ x = x` / `x ∨ x = x`：K3 下 `U ∧ U = U`，Ł3 下 `U ∧ U = F`，
//! 幂等律本来就不成立；也不碰 `=>` / `<=>` 的常量律，K3 与 Ł3 的蕴含表不同。
//! 吸收律 `x ∧ F = F` / `x ∨ T = T` 也不做：B3 的 U 传染，`U ∧ F = U`。
//! 规则本身由 `tests.rs` 对 3 种代数 × 3 个值逐条穷举。

use trit_core::value::Ternary;

use super::super::ir::Op;
use super::super::lower::prune_dead;

pub(super) fn simplify(ops: Vec<Op>) -> Vec<Op> {
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
                if consts[left] == Some(Ternary::True) {
                    resolve.push(right);
                } else if consts[right] == Some(Ternary::True) {
                    resolve.push(left);
                } else {
                    push_node(&mut resolve, &mut kept, &mut consts, Op::And(left, right));
                }
            }
            Op::Or(left, right) => {
                let (left, right) = (resolve[left], resolve[right]);
                if consts[left] == Some(Ternary::False) {
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

fn push_const(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    value: Ternary,
) {
    resolve.push(kept.len());
    kept.push(Op::Const(value));
    consts.push(Some(value));
}

fn push_node(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    op: Op,
) {
    resolve.push(kept.len());
    kept.push(op);
    consts.push(None);
}

fn push_binary(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    left: usize,
    right: usize,
    build: fn(usize, usize) -> Op,
) {
    let left = resolve[left];
    let right = resolve[right];
    push_node(resolve, kept, consts, build(left, right));
}

#[cfg(test)]
mod tests;
