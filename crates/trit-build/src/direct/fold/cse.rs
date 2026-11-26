//! 结构化 CSE：化简把操作数别名掉以后，原本不同的节点可能变成同构子式。
//! 这里按「算子 + 已重映射的子下标」做一次哈希合并，交换律算子先排序；
//! `And` / `Or` 另外做结合链展平、去重与排序（结合律在三代数下都成立），
//! 最后再按根收孤儿。

use std::collections::HashMap;

use trit_core::value::Ternary;

use crate::direct::ir::Op;
use crate::direct::lower::prune_dead;
mod assoc;

type Key = (u8, usize, usize);

pub(super) fn cse(ops: Vec<Op>) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let root = ops.len() - 1;
    let mut remap = vec![usize::MAX; ops.len()];
    let mut kept = Vec::with_capacity(ops.len());
    let mut memo: HashMap<Key, usize> = HashMap::new();
    let mut chains: HashMap<assoc::Key, usize> = HashMap::new();
    for (index, op) in ops.into_iter().enumerate() {
        let op = normalize(remap_op(op, &remap));
        if let Op::And(left, right) | Op::Or(left, right) = op {
            let dual = matches!(op, Op::Or(..));
            remap[index] = assoc::norm(&mut kept, &mut chains, dual, left, right);
            continue;
        }
        let key = key_of(op);
        let out = match memo.get(&key) {
            Some(&out) => out,
            None => {
                let out = kept.len();
                kept.push(op);
                memo.insert(key, out);
                out
            }
        };
        remap[index] = out;
    }
    prune_dead(kept, remap[root])
}

fn remap_op(op: Op, map: &[usize]) -> Op {
    match op {
        Op::Const(value) => Op::Const(value),
        Op::Var(slot) => Op::Var(slot),
        Op::Not(inner) => Op::Not(map[inner]),
        Op::And(left, right) => Op::And(map[left], map[right]),
        Op::Or(left, right) => Op::Or(map[left], map[right]),
        Op::Xor(left, right) => Op::Xor(map[left], map[right]),
        Op::Xor2(left, right) => Op::Xor2(map[left], map[right]),
        Op::Implies(left, right) => Op::Implies(map[left], map[right]),
        Op::Equiv(left, right) => Op::Equiv(map[left], map[right]),
    }
}

fn normalize(op: Op) -> Op {
    match op {
        Op::Xor(a, b) => Op::Xor(a.min(b), a.max(b)),
        Op::Xor2(a, b) => Op::Xor2(a.min(b), a.max(b)),
        Op::Equiv(a, b) => Op::Equiv(a.min(b), a.max(b)),
        other => other,
    }
}

fn key_of(op: Op) -> Key {
    match op {
        Op::Const(Ternary::False) => (0, 0, 0),
        Op::Const(Ternary::Unknown) => (0, 1, 0),
        Op::Const(Ternary::True) => (0, 2, 0),
        Op::Var(slot) => (1, slot, 0),
        Op::Not(inner) => (2, inner, 0),
        Op::And(..) | Op::Or(..) => unreachable!("And/Or 走结合链"),
        Op::Xor(left, right) => (5, left, right),
        Op::Xor2(left, right) => (6, left, right),
        Op::Implies(left, right) => (7, left, right),
        Op::Equiv(left, right) => (8, left, right),
    }
}

#[cfg(test)]
mod tests;
