//! 结构化 CSE：化简把操作数别名掉以后，原本不同的节点可能变成同构子式。
//! 这里按「算子 + 已重映射的子下标」做一次哈希合并，交换律算子先排序；
//! `And` / `Or` 做结合链展平、去重与排序，`Xor2` 只做展平与排序（不做幂等去重）。
//! 两遍之间提取公因子，最后再按根收孤儿。

use crate::direct::ir::Op;
mod assoc;
mod factor;
mod key;
mod pass;
mod xorchain;
use pass::pass;

pub(super) fn cse(ops: Vec<Op>) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let ops = pass(ops);
    // 提取公因子前先合并同构节点，命中后再压平新建的内层链。
    let ops = factor::factor(ops);
    pass(ops)
}

#[cfg(test)]
mod tests;
