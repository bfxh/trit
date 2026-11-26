//! `Xor2`（Verilog 经典异或）的结合链规范化：展平、排序、重建左深链。
//! `XorClassic` 在三种代数下都满足结合律：任一操作数为 `U` 时结果必为 `U`，
//! 全已知时就是布尔异或。重复叶子不能按幂等律去掉——`x ^ x` 在 `x = U` 时仍是 `U`，
//! 不是 `F`，所以这里只共享前缀，不去重。

use std::collections::{HashMap, HashSet};

use crate::direct::ir::Op;

pub(super) type Key = Vec<usize>;

pub(super) fn norm(
    kept: &mut Vec<Op>,
    memo: &mut HashMap<Key, usize>,
    left: usize,
    right: usize,
) -> usize {
    let mut leaves = Vec::new();
    let mut expanded = HashSet::new();
    collect(kept, left, &mut expanded, &mut leaves);
    collect(kept, right, &mut expanded, &mut leaves);
    leaves.sort_unstable();
    if leaves.len() == 1 {
        return leaves[0];
    }
    let mut acc = leaves[0];
    for end in 2..=leaves.len() {
        let key = leaves[..end].to_vec();
        if let Some(&node) = memo.get(&key) {
            acc = node;
            continue;
        }
        let node = kept.len();
        kept.push(Op::Xor2(acc, leaves[end - 1]));
        memo.insert(key, node);
        acc = node;
    }
    acc
}

fn collect(kept: &[Op], index: usize, expanded: &mut HashSet<usize>, out: &mut Vec<usize>) {
    match kept[index] {
        Op::Xor2(left, right) if expanded.insert(index) => {
            collect(kept, left, expanded, out);
            collect(kept, right, expanded, out);
        }
        _ => out.push(index),
    }
}
