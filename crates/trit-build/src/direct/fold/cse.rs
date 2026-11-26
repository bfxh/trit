//! 结构化 CSE：化简把操作数别名掉以后，原本不同的节点可能变成同构子式。
//! 这里按「算子 + 已重映射的子下标」做一次哈希合并，交换律算子先排序；
//! `And` / `Or` 另外做结合链展平、去重与排序（结合律在三代数下都成立），
//! 两遍之间提取公因子（`(a ∧ b) ∨ (a ∧ c) = a ∧ (b ∨ c)`），最后再按根收孤儿。

use std::collections::HashMap;

use crate::direct::ir::Op;
use crate::direct::lower::prune_dead;
mod assoc;
mod factor;
mod key;
use key::{Key, key_of, normalize, remap_op};

pub(super) fn cse(ops: Vec<Op>) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let ops = pass(ops);
    // 提取公因子前先合并同构节点，命中后再压平新建的内层链。
    let ops = factor::factor(ops);
    pass(ops)
}

fn pass(ops: Vec<Op>) -> Vec<Op> {
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

#[cfg(test)]
mod tests;
