//! 分组尺度下的精确补丁搜索。
//!
//! 本仓的 `repair` 建立在一个前提上：单点编辑的效果**只取决于 trit 自身**（`Layer` 的 scale
//! 是每行一个正数，判等可忽略 scale）。`GroupedLayer` 没有这个性质——同一行内不同列可能落在
//! 不同 scale 上，于是**同一个 trit 编辑的效果取决于它属于哪一组**。
//!
//! 本模块给 `GroupedLayer` 提供单编辑补丁搜索，并把这个差异测出来：
//!
//! - [`repair_single`]：枚举 `(row, col, 新 trit)`，找第一个能把 `loss` 降到 0 的单点编辑；
//! - [`loss`]：与 `Oracle` 的失配比例（按符号比较，与仓库"行为判等只看符号"一致）。
//!
//! 注意搜索空间：`3 · rows · cols` 个候选，只适合小层——与 `repair` 的定位一致。

use crate::error::Result;
use crate::grouped::GroupedLayer;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 单个 trit 编辑构成的补丁。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TritEdit {
    /// 行。
    pub row: usize,
    /// 列。
    pub col: usize,
    /// 编辑前的 trit。
    pub from: Trit,
    /// 编辑后的 trit。
    pub to: Trit,
}

/// 与 `Oracle` 的失配比例（0.0 表示完全匹配）。
pub fn loss(layer: &GroupedLayer, oracle: &Oracle) -> Result<f32> {
    let inputs = oracle.inputs();
    let outputs = oracle.outputs();
    if inputs.is_empty() {
        return Ok(0.0);
    }
    let mut wrong = 0usize;
    for (input, expected) in inputs.iter().zip(outputs) {
        let features: Vec<f32> = input.iter().map(|trit| f32::from(trit.to_i8())).collect();
        let out = layer.forward(&features)?;
        for (value, want) in out.iter().zip(expected) {
            let sign = if *value > 1e-9 {
                Trit::Pos
            } else if *value < -1e-9 {
                Trit::Neg
            } else {
                Trit::Zero
            };
            if sign != *want {
                wrong += 1;
            }
        }
    }
    Ok(wrong as f32 / (inputs.len() * outputs[0].len()) as f32)
}

fn with_edit(layer: &GroupedLayer, row: usize, col: usize, to: Trit) -> Result<GroupedLayer> {
    let mut trits: Vec<i8> = layer.trits.iter().map(|trit| trit.to_i8()).collect();
    trits[row * layer.cols() + col] = to.to_i8();
    GroupedLayer::from_trits(
        layer.rows(),
        layer.cols(),
        &trits,
        &layer.groups,
        &layer.scales,
        &layer.bias_f32,
    )
}

/// 枚举单点编辑，返回第一个能完全修复的补丁。
///
/// 返回 `Ok(None)` 表示单编辑无解（需要更多编辑，或该层根本表达不了这个 oracle）。
pub fn repair_single(layer: &GroupedLayer, oracle: &Oracle) -> Result<Option<TritEdit>> {
    if loss(layer, oracle)? == 0.0 {
        return Ok(None);
    }
    for row in 0..layer.rows() {
        for col in 0..layer.cols() {
            let from = match layer.trit(row, col) {
                Some(trit) => trit,
                None => continue,
            };
            for to in Trit::ALL {
                if to == from {
                    continue;
                }
                let candidate = with_edit(layer, row, col, to)?;
                if loss(&candidate, oracle)? == 0.0 {
                    return Ok(Some(TritEdit { row, col, from, to }));
                }
            }
        }
    }
    Ok(None)
}

/// 施加一个编辑，得到新层（不改原层）。
pub fn apply(layer: &GroupedLayer, edit: &TritEdit) -> Result<GroupedLayer> {
    with_edit(layer, edit.row, edit.col, edit.to)
}
