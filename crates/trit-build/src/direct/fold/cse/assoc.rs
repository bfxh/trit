//! `And` / `Or` 的结合链规范化：把嵌套的同一算子展平成叶子、去重、排序，
//! 再重建为左深链。交换律与结合律在三代数下都成立，所以 `(a & b) & c`
//! 与 `c & (a & b)` 会落到同一条链；重复叶子按幂等律去掉。

use std::collections::HashMap;

use crate::direct::ir::Op;

pub(super) type Key = (bool, Vec<usize>);

pub(super) fn norm(
    kept: &mut Vec<Op>,
    memo: &mut HashMap<Key, usize>,
    dual: bool,
    left: usize,
    right: usize,
) -> usize {
    let mut leaves = Vec::new();
    collect(kept, left, dual, &mut leaves);
    collect(kept, right, dual, &mut leaves);
    leaves.sort_unstable();
    leaves.dedup();
    if leaves.len() == 1 {
        return leaves[0];
    }
    let mut acc = leaves[0];
    for end in 2..=leaves.len() {
        let key = (dual, leaves[..end].to_vec());
        if let Some(&node) = memo.get(&key) {
            acc = node;
            continue;
        }
        let node = kept.len();
        kept.push(if dual {
            Op::Or(acc, leaves[end - 1])
        } else {
            Op::And(acc, leaves[end - 1])
        });
        memo.insert(key, node);
        acc = node;
    }
    acc
}

fn collect(kept: &[Op], index: usize, dual: bool, out: &mut Vec<usize>) {
    match kept[index] {
        Op::And(left, right) if !dual => {
            collect(kept, left, dual, out);
            collect(kept, right, dual, out);
        }
        Op::Or(left, right) if dual => {
            collect(kept, left, dual, out);
            collect(kept, right, dual, out);
        }
        _ => out.push(index),
    }
}
