//! 权重空间代数：对齐（消掉隐藏单元置换对称性）与符号投票合并。
//!
//! 三值网络的隐藏单元存在置换对称性：交换第 `l` 层的两行并同时交换第 `l+1`
//! 层的两列，行为完全不变。所以直接逐点平均会**把两个等价模型合坏**；先对齐再合并。

use crate::error::{Error, Result};
use crate::net::{Layer, Net};
use crate::trit::Trit;

/// 把 `other` 的隐藏层行置换到与 `reference` 对齐（只动隐藏层，输出层不动）。
pub fn align(reference: &Net, other: &Net) -> Result<Net> {
    if reference.arch() != other.arch() {
        return Err(Error::ArchMismatch);
    }
    let mut current = other.clone();
    for layer in 0..current.depth().saturating_sub(1) {
        let perm = greedy_match(
            reference.layer(layer).expect("arch checked"),
            current.layer(layer).expect("arch checked"),
        );
        current = current.permute_rows(layer, &perm)?;
    }
    Ok(current)
}

/// 逐点符号投票：`+1` 记 `+1`、`-1` 记 `-1`、正负抵消记 `0`。
pub fn merge_vote(left: &Net, right: &Net) -> Result<Net> {
    if left.arch() != right.arch() {
        return Err(Error::ArchMismatch);
    }
    let mut layers = Vec::with_capacity(left.depth());
    for (a, b) in left.layers().iter().zip(right.layers()) {
        let trits: Vec<i8> = a
            .trits()
            .iter()
            .zip(b.trits())
            .map(|(&x, &y)| Trit::sign_of(i32::from(x.to_i8()) + i32::from(y.to_i8())).to_i8())
            .collect();
        let bias: Vec<i8> = a
            .bias()
            .iter()
            .zip(b.bias())
            .map(|(&x, &y)| Trit::sign_of(i32::from(x.to_i8()) + i32::from(y.to_i8())).to_i8())
            .collect();
        let scales: Vec<f32> = a
            .scales()
            .iter()
            .zip(b.scales())
            .map(|(&x, &y)| (x + y) * 0.5)
            .collect();
        layers.push(Layer::with_scales(
            a.rows(),
            a.cols(),
            &trits,
            &bias,
            scales,
        )?);
    }
    Net::new(layers)
}

/// 先对齐再投票；对“同一个模型换了隐藏单元编号”的情形应还原出原模型。
pub fn merge_aligned(left: &Net, right: &Net) -> Result<Net> {
    let aligned = align(left, right)?;
    merge_vote(left, &aligned)
}

/// 贪心行匹配：`perm[i]` 是“作为新第 `i` 行”的旧行号。
fn greedy_match(reference: &Layer, other: &Layer) -> Vec<usize> {
    let rows = reference.rows();
    let mut used = vec![false; rows];
    let mut perm = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut best = usize::MAX;
        let mut best_score = 0usize;
        for (candidate, &taken) in used.iter().enumerate() {
            if taken {
                continue;
            }
            let score = agreement(reference, row, other, candidate);
            if best == usize::MAX || score > best_score {
                best = candidate;
                best_score = score;
            }
        }
        used[best] = true;
        perm.push(best);
    }
    perm
}

fn agreement(reference: &Layer, row: usize, other: &Layer, candidate: usize) -> usize {
    let left = reference.row(row).expect("row in range");
    let right = other.row(candidate).expect("row in range");
    let weights = left.iter().zip(right).filter(|(a, b)| a == b).count();
    let bias = usize::from(
        reference.bias_trit(row).expect("row in range")
            == other.bias_trit(candidate).expect("row in range"),
    );
    weights + bias
}
