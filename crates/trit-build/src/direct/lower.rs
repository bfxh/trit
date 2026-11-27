//! 表达式 → 线性 IR，以及两遍整理：交换律规范化与死代码消除。
//!
//! - 交换律：`∧ ∨ ⊕ ≡` 在三种代数下都对两个操作数对称（`tests.rs` 对 3×3 表逐格对拍），
//!   所以子节点按 IR 下标排序后再查 CSE 表：`b & a` 与 `a & b` 共用一个节点。
//! - 死代码消除：从根反向标记可达节点，再按原顺序压缩、重编号。
//!   当前 lowering 不会产生孤儿节点；这一遍是给后续重写/常量折叠留的兜底，
//!   行为由合成 IR 的测试钉住。

use std::collections::HashMap;

use anyhow::{Context, Result};
use trit_core::Expr;
use trit_core::value::Ternary;

use super::ir::Op;

/// 表达式 → 线性 IR（后序，最后一条即根）。同构子式走 CSE，只算一次。
pub(super) fn lower(expr: &Expr, vars: &[String]) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let mut memo = HashMap::new();
    push(expr, vars, &mut ops, &mut memo)?;
    let root = ops.len().saturating_sub(1);
    Ok(prune_dead(ops, root))
}

/// 去重键：`(算子, 左子, 右子)`，一元/常量/变量的右子位补 0。
type Key = (u8, usize, usize);

fn intern(ops: &mut Vec<Op>, memo: &mut HashMap<Key, usize>, key: Key, op: Op) -> usize {
    if let Some(index) = memo.get(&key) {
        return *index;
    }
    ops.push(op);
    let index = ops.len() - 1;
    memo.insert(key, index);
    index
}

/// 交换律算子的规范化入口：先把两个子节点按 IR 下标排好，再查 CSE。
fn commutative(
    ops: &mut Vec<Op>,
    memo: &mut HashMap<Key, usize>,
    code: u8,
    left: usize,
    right: usize,
    build: fn(usize, usize) -> Op,
) -> usize {
    let (low, high) = if left <= right {
        (left, right)
    } else {
        (right, left)
    };
    intern(ops, memo, (code, low, high), build(low, high))
}

fn push(
    expr: &Expr,
    vars: &[String],
    ops: &mut Vec<Op>,
    memo: &mut HashMap<Key, usize>,
) -> Result<usize> {
    Ok(match expr {
        Expr::Const(value) => {
            let slot = ordinal(*value);
            intern(ops, memo, (0, slot, 0), Op::Const(*value))
        }
        Expr::Var(name) => {
            let slot = vars
                .iter()
                .position(|var| var == name)
                .with_context(|| format!("变量 {name:?} 不在变量表 {vars:?} 里"))?;
            intern(ops, memo, (1, slot, 0), Op::Var(slot))
        }
        Expr::Not(inner) => {
            let inner = push(inner, vars, ops, memo)?;
            intern(ops, memo, (2, inner, 0), Op::Not(inner))
        }
        Expr::And(left, right) => {
            let left = push(left, vars, ops, memo)?;
            let right = push(right, vars, ops, memo)?;
            commutative(ops, memo, 3, left, right, Op::And)
        }
        Expr::Or(left, right) => {
            let left = push(left, vars, ops, memo)?;
            let right = push(right, vars, ops, memo)?;
            commutative(ops, memo, 4, left, right, Op::Or)
        }
        Expr::Xor(left, right) => {
            let left = push(left, vars, ops, memo)?;
            let right = push(right, vars, ops, memo)?;
            commutative(ops, memo, 5, left, right, Op::Xor)
        }
        Expr::Implies(left, right) => {
            let left = push(left, vars, ops, memo)?;
            let right = push(right, vars, ops, memo)?;
            intern(ops, memo, (6, left, right), Op::Implies(left, right))
        }
        Expr::Equiv(left, right) => {
            let left = push(left, vars, ops, memo)?;
            let right = push(right, vars, ops, memo)?;
            commutative(ops, memo, 7, left, right, Op::Equiv)
        }
    })
}

fn ordinal(value: Ternary) -> usize {
    match value {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

/// 从根反向标记可达节点，丢掉孤儿并把下标压紧；根仍在最后一条。
pub(super) fn prune_dead(ops: Vec<Op>, root: usize) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let mut live = vec![false; ops.len()];
    let mut stack = vec![root];
    while let Some(index) = stack.pop() {
        if live[index] {
            continue;
        }
        live[index] = true;
        match ops[index] {
            Op::Const(_) | Op::Var(_) => {}
            Op::Not(inner) => stack.push(inner),
            Op::And(left, right)
            | Op::Or(left, right)
            | Op::Xor(left, right)
            | Op::Xor2(left, right)
            | Op::Implies(left, right)
            | Op::Equiv(left, right) => {
                stack.push(left);
                stack.push(right);
            }
        }
    }
    let mut remap = vec![usize::MAX; ops.len()];
    let mut kept = Vec::with_capacity(live.iter().filter(|entry| **entry).count());
    for (index, op) in ops.iter().copied().enumerate() {
        if !live[index] {
            continue;
        }
        let mapped = match op {
            Op::Const(value) => Op::Const(value),
            Op::Var(slot) => Op::Var(slot),
            Op::Not(inner) => Op::Not(remap[inner]),
            Op::And(left, right) => Op::And(remap[left], remap[right]),
            Op::Or(left, right) => Op::Or(remap[left], remap[right]),
            Op::Xor(left, right) => Op::Xor(remap[left], remap[right]),
            Op::Xor2(left, right) => Op::Xor2(remap[left], remap[right]),
            Op::Implies(left, right) => Op::Implies(remap[left], remap[right]),
            Op::Equiv(left, right) => Op::Equiv(remap[left], remap[right]),
        };
        remap[index] = kept.len();
        kept.push(mapped);
    }
    kept
}

#[cfg(test)]
mod tests;
