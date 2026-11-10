//! DARE 的三值对应物：按概率丢弃 `base → tuned` 的差分 trit，保留部分照常落位。
//!
//! 与 TIES 任务向量的衔接：[`crate::task`] 先求差分、按坐标投票合并；本模块在
//! 差分上做 DARE 式随机丢弃（drop），降低多任务合并时的坐标冲突密度。
//!
//! 三值差分只有 `-1/0/+1`，DARE 的 `1/(1-rate)` 重标度没法写进 trit；这里把它
//! 吸收进**有保留差分的行**的 scale：scale 是正标尺自由度，只改 f32 读数、
//! 不改符号行为，所以重标度对 [`crate::Oracle`] 判决是中性记录。
//!
//! 丢弃决定由 splitmix64 对 `(seed, layer, row, col)` 做确定性散列得出：
//! 同种子同坐标永远同结果，补丁可复现、可审计。偏置坐标用 `col = usize::MAX`
//! 标记（与 [`crate::task`] 的投票键一致）；[`dare_drop_rows`] 按整行决策，
//! 用 `col = usize::MAX - 1` 作行标记，与偏置坐标区分。

use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;

/// 行级决策的列标记（与偏置的 `usize::MAX` 区分）。
const ROW_UNIT: usize = usize::MAX - 1;

/// 层级的决策标记（与行级、偏置区分）。
const LAYER_UNIT: usize = usize::MAX - 2;

/// 丢弃的决策单位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DareMode {
    /// 每个差分坐标独立。
    Coord,
    /// 整行（trit + 偏置）一起。
    Row,
    /// 按固定列块（`col / block`）一起。
    Block(usize),
    /// 整层一起。
    Layer,
}

/// 丢弃结果：补丁 + 读数。
#[derive(Clone, Debug, PartialEq)]
pub struct DropOutcome {
    /// 应用到 `base` 上得到“丢弃后的 tuned”的补丁。
    pub patch: Patch,
    /// 被丢回基准值的差分坐标数。
    pub dropped: usize,
    /// 保留并落位的差分坐标数（trit + 偏置）。
    pub kept: usize,
    /// 被乘以 `1/(1-rate)` 重标度的行数。
    pub rescaled_rows: usize,
}

/// 对 `base → tuned` 的差分做 DARE 丢弃（两者必须同架构）。
///
/// `rate` 是丢弃概率，必须在 `[0, 1)`：`0` 等价于完整差分（不重标度），
/// 越接近 `1` 丢得越多。`rate = 1` 会把重标度变成除零，直接判错。
/// 返回的补丁只含 `SetTrit` / `SetBias` / `SetScale`，可走事务、审计、三方重放。
///
/// 这是**坐标级**版本：每个差分坐标独立决定保留或丢弃。
pub fn dare_drop(base: &Net, tuned: &Net, rate: f32, seed: u64) -> Result<DropOutcome> {
    dare_drop_impl(base, tuned, rate, seed, DareMode::Coord)
}

/// 行级 DARE：整行（trit + 偏置）一起保留或一起丢回基准值。
///
/// 决策单位是 `(seed, layer, row)`：一行只要有任何 trit/偏置差分，就整行保留或
/// 整行丢弃，不会出现“半行保留”。`kept` / `dropped` 仍按**差分坐标数**计数。
/// 只改本层该行的入边与偏置，不碰下一层的出边列（单元粒度整体搬运见
/// [`crate::transplant`]）。
pub fn dare_drop_rows(base: &Net, tuned: &Net, rate: f32, seed: u64) -> Result<DropOutcome> {
    dare_drop_impl(base, tuned, rate, seed, DareMode::Row)
}

/// 层 DARE：整层（所有行、所有 trit 与偏置）一起保留或一起丢弃。
///
/// 决策单位是 `(seed, layer)`；`kept` / `dropped` 仍按差分坐标数计数，重标度只落在
/// 有保留差分的行上。
pub fn dare_drop_layers(base: &Net, tuned: &Net, rate: f32, seed: u64) -> Result<DropOutcome> {
    dare_drop_impl(base, tuned, rate, seed, DareMode::Layer)
}

/// 块级 DARE：同一行内按固定列块（`col / block`）一起保留或一起丢弃。
///
/// 偏置仍按坐标级单独决策；`block = 1` 退化为坐标级。`block = 0` 判错。
/// `kept` / `dropped` 仍按差分坐标数计数。
pub fn dare_drop_blocks(
    base: &Net,
    tuned: &Net,
    rate: f32,
    seed: u64,
    block: usize,
) -> Result<DropOutcome> {
    if block == 0 {
        return Err(Error::InvalidBlock { block });
    }
    dare_drop_impl(base, tuned, rate, seed, DareMode::Block(block))
}

fn dare_drop_impl(
    base: &Net,
    tuned: &Net,
    rate: f32,
    seed: u64,
    mode: DareMode,
) -> Result<DropOutcome> {
    if base.arch() != tuned.arch() {
        return Err(Error::ArchMismatch);
    }
    if !rate.is_finite() || !(0.0..1.0).contains(&rate) {
        return Err(Error::InvalidRate { rate });
    }
    let factor = 1.0 / (1.0 - rate);
    let mut edits = Vec::new();
    let mut dropped = 0usize;
    let mut kept = 0usize;
    let mut rescaled_rows = 0usize;
    for (layer, (src, dst)) in base.layers().iter().zip(tuned.layers()).enumerate() {
        if matches!(mode, DareMode::Layer) {
            let mut layer_edits = Vec::new();
            let mut deltas = 0usize;
            for row in 0..src.rows() {
                for col in 0..src.cols() {
                    let from = src.trit(row, col).expect("row/col in range");
                    let to = dst.trit(row, col).expect("row/col in range");
                    if from != to {
                        layer_edits.push(Edit::SetTrit {
                            layer,
                            row,
                            col,
                            to,
                        });
                        deltas += 1;
                    }
                }
                let from_bias = src.bias_trit(row).expect("row in range");
                let to_bias = dst.bias_trit(row).expect("row in range");
                if from_bias != to_bias {
                    layer_edits.push(Edit::SetBias {
                        layer,
                        row,
                        to: to_bias,
                    });
                    deltas += 1;
                }
            }
            if deltas == 0 {
                continue;
            }
            let mut layer_rescaled = 0usize;
            if rate > 0.0 {
                for row in 0..src.rows() {
                    let row_diff = (0..src.cols())
                        .any(|col| src.trit(row, col) != dst.trit(row, col))
                        || src.bias_trit(row) != dst.bias_trit(row);
                    if !row_diff {
                        continue;
                    }
                    let scale = dst.scale(row).expect("row in range");
                    let rescaled = scale * factor;
                    if rescaled.to_bits() != scale.to_bits() {
                        layer_edits.push(Edit::SetScale {
                            layer,
                            row,
                            to: rescaled,
                        });
                        layer_rescaled += 1;
                    }
                }
            }
            if keep(seed, layer, LAYER_UNIT, LAYER_UNIT, rate) {
                kept += deltas;
                rescaled_rows += layer_rescaled;
                edits.extend(layer_edits);
            } else {
                dropped += deltas;
            }
            continue;
        }
        for row in 0..src.rows() {
            if matches!(mode, DareMode::Row) {
                let mut row_edits = Vec::new();
                for col in 0..src.cols() {
                    let from = src.trit(row, col).expect("row/col in range");
                    let to = dst.trit(row, col).expect("row/col in range");
                    if from != to {
                        row_edits.push(Edit::SetTrit {
                            layer,
                            row,
                            col,
                            to,
                        });
                    }
                }
                let from_bias = src.bias_trit(row).expect("row in range");
                let to_bias = dst.bias_trit(row).expect("row in range");
                if from_bias != to_bias {
                    row_edits.push(Edit::SetBias {
                        layer,
                        row,
                        to: to_bias,
                    });
                }
                if row_edits.is_empty() {
                    continue;
                }
                if keep(seed, layer, row, ROW_UNIT, rate) {
                    kept += row_edits.len();
                    edits.extend(row_edits);
                    if rate > 0.0 {
                        let scale = dst.scale(row).expect("row in range");
                        let rescaled = scale * factor;
                        if rescaled.to_bits() != scale.to_bits() {
                            edits.push(Edit::SetScale {
                                layer,
                                row,
                                to: rescaled,
                            });
                            rescaled_rows += 1;
                        }
                    }
                } else {
                    dropped += row_edits.len();
                }
                continue;
            }
            let mut kept_any = false;
            for col in 0..src.cols() {
                let from = src.trit(row, col).expect("row/col in range");
                let to = dst.trit(row, col).expect("row/col in range");
                if from == to {
                    continue;
                }
                let unit_col = match mode {
                    DareMode::Coord => col,
                    DareMode::Block(block) => col / block,
                    DareMode::Row => unreachable!(),
                    DareMode::Layer => unreachable!(),
                };
                if keep(seed, layer, row, unit_col, rate) {
                    edits.push(Edit::SetTrit {
                        layer,
                        row,
                        col,
                        to,
                    });
                    kept += 1;
                    kept_any = true;
                } else {
                    dropped += 1;
                }
            }
            let from_bias = src.bias_trit(row).expect("row in range");
            let to_bias = dst.bias_trit(row).expect("row in range");
            if from_bias != to_bias {
                if keep(seed, layer, row, usize::MAX, rate) {
                    edits.push(Edit::SetBias {
                        layer,
                        row,
                        to: to_bias,
                    });
                    kept += 1;
                    kept_any = true;
                } else {
                    dropped += 1;
                }
            }
            if kept_any && rate > 0.0 {
                let scale = dst.scale(row).expect("row in range");
                let rescaled = scale * factor;
                if rescaled.to_bits() != scale.to_bits() {
                    edits.push(Edit::SetScale {
                        layer,
                        row,
                        to: rescaled,
                    });
                    rescaled_rows += 1;
                }
            }
        }
    }
    Ok(DropOutcome {
        patch: Patch::new(edits),
        dropped,
        kept,
        rescaled_rows,
    })
}

pub(crate) fn keep(seed: u64, layer: usize, row: usize, col: usize, rate: f32) -> bool {
    let mixed = seed
        ^ (layer as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (row as u64).wrapping_mul(0xD1B5_4A32_D192_ED03)
        ^ (col as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    let hash = splitmix64(mixed);
    let unit = (hash >> 11) as f64 / 9_007_199_254_740_992.0;
    unit >= f64::from(rate)
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
