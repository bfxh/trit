//! 三方补丁重放：把针对 `base` 写的补丁落到已经前进到 `target` 的工件上。
//!
//! 每条编辑先比较 `base` 与 `target` 在同一坐标上的状态：
//!
//! - 两边相同 ⇒ 这条编辑可以应用；
//! - `target` 已经是补丁想要的值（行操作则是已经做过）⇒ 记为已应用并跳过；
//! - 两边不同且不是已应用 ⇒ 冲突，绝不静默覆盖。
//!
//! 冲突按**整体**判定：没有开 `skip_conflicts` 时 `rejected=true`，调用方不得应用
//! 返回的 [`RebaseReport::clean`]；开了才只落干净的那部分，冲突仍逐条报告。

use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::{Layer, Net};

/// 一次重放的分类结果（不改任何工件）。
#[derive(Clone, Debug)]
pub struct RebaseReport {
    /// 与 `target` 不冲突、需要应用的编辑（保持原顺序）。
    pub clean: Patch,
    /// 冲突列表（按补丁顺序）。
    pub conflicts: Vec<Conflict>,
    /// `target` 上已经是目标值、无需再应用的编辑条数。
    pub already_applied: usize,
    /// 有冲突且未允许跳过；此时 `clean` 不得应用。
    pub rejected: bool,
}

/// 一条冲突：补丁想把 `base` 上的值改成目标值，但 `target` 已经不一样了。
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    /// 冲突的编辑。
    pub edit: Edit,
    /// `base` 上被这条编辑碰到的状态（点编辑是三值字符，scale 是数值，行操作是行指纹）。
    pub base: String,
    /// `target` 上同一位置的状态（格式同 `base`）。
    pub target: String,
}

enum Verdict {
    Clean,
    AlreadyApplied,
    Conflict { base: String, target: String },
}

/// 三方重放分类。`skip_conflicts=false` 时只要有一条冲突就 `rejected=true`。
///
/// `base` 是补丁写作时的工件，`target` 是现在要落到的工件；两者必须同架构。
pub fn rebase(
    base: &Net,
    target: &Net,
    patch: &Patch,
    skip_conflicts: bool,
) -> Result<RebaseReport> {
    if base.arch() != target.arch() {
        return Err(Error::ArchMismatch);
    }
    patch.validate(base)?;
    let mut clean = Vec::new();
    let mut conflicts = Vec::new();
    let mut already_applied = 0;
    for edit in patch.edits() {
        match classify(base, target, edit)? {
            Verdict::Clean => clean.push(edit.clone()),
            Verdict::AlreadyApplied => already_applied += 1,
            Verdict::Conflict { base, target } => conflicts.push(Conflict {
                edit: edit.clone(),
                base,
                target,
            }),
        }
    }
    let rejected = !conflicts.is_empty() && !skip_conflicts;
    Ok(RebaseReport {
        clean: Patch::new(clean),
        conflicts,
        already_applied,
        rejected,
    })
}

fn classify(base: &Net, target: &Net, edit: &Edit) -> Result<Verdict> {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
        } => {
            let before = trit_at(base, layer, row, col)?;
            let now = trit_at(target, layer, row, col)?;
            if before == now {
                Ok(Verdict::Clean)
            } else if now == to {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: before.to_string(),
                    target: now.to_string(),
                })
            }
        }
        Edit::SetBias { layer, row, to } => {
            let before = bias_at(base, layer, row)?;
            let now = bias_at(target, layer, row)?;
            if before == now {
                Ok(Verdict::Clean)
            } else if now == to {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: before.to_string(),
                    target: now.to_string(),
                })
            }
        }
        Edit::SetScale { layer, row, to } => {
            let before = scale_at(base, layer, row)?;
            let now = scale_at(target, layer, row)?;
            if before.to_bits() == now.to_bits() {
                Ok(Verdict::Clean)
            } else if now.to_bits() == to.to_bits() {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: before.to_string(),
                    target: now.to_string(),
                })
            }
        }
        Edit::SwapRows { layer, a, b } => {
            if a == b {
                return Ok(Verdict::Clean);
            }
            let before = layer_at(base, layer)?;
            let now = layer_at(target, layer)?;
            if row_eq(before, a, now, a) && row_eq(before, b, now, b) {
                Ok(Verdict::Clean)
            } else if row_eq(before, a, now, b) && row_eq(before, b, now, a) {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: row_pair(before, a, b),
                    target: row_pair(now, a, b),
                })
            }
        }
        Edit::NegateRow { layer, row } => {
            let before = layer_at(base, layer)?;
            let now = layer_at(target, layer)?;
            if row_eq(before, row, now, row) {
                Ok(Verdict::Clean)
            } else if row_eq_negated(before, row, now, row) {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: row_fingerprint(before, row),
                    target: row_fingerprint(now, row),
                })
            }
        }
    }
}

fn layer_at(net: &Net, layer: usize) -> Result<&Layer> {
    net.layer(layer).ok_or(Error::OutOfBounds {
        layer,
        row: 0,
        col: 0,
    })
}

fn trit_at(net: &Net, layer: usize, row: usize, col: usize) -> Result<crate::trit::Trit> {
    layer_at(net, layer)?
        .trit(row, col)
        .ok_or(Error::OutOfBounds { layer, row, col })
}

fn bias_at(net: &Net, layer: usize, row: usize) -> Result<crate::trit::Trit> {
    layer_at(net, layer)?
        .bias_trit(row)
        .ok_or(Error::OutOfBounds { layer, row, col: 0 })
}

fn scale_at(net: &Net, layer: usize, row: usize) -> Result<f32> {
    layer_at(net, layer)?
        .scale(row)
        .ok_or(Error::OutOfBounds { layer, row, col: 0 })
}

fn row_eq(left: &Layer, left_row: usize, right: &Layer, right_row: usize) -> bool {
    left.row(left_row) == right.row(right_row)
        && left.bias_trit(left_row) == right.bias_trit(right_row)
        && left.scale(left_row).map(f32::to_bits) == right.scale(right_row).map(f32::to_bits)
}

fn row_eq_negated(left: &Layer, left_row: usize, right: &Layer, right_row: usize) -> bool {
    let same_weights = match (left.row(left_row), right.row(right_row)) {
        (Some(a), Some(b)) => a.iter().map(|trit| trit.neg()).eq(b.iter().copied()),
        _ => false,
    };
    same_weights
        && left.bias_trit(left_row).map(|trit| trit.neg()) == right.bias_trit(right_row)
        && left.scale(left_row).map(f32::to_bits) == right.scale(right_row).map(f32::to_bits)
}

fn row_pair(layer: &Layer, a: usize, b: usize) -> String {
    format!(
        "{}/{}",
        row_fingerprint(layer, a),
        row_fingerprint(layer, b)
    )
}

fn row_fingerprint(layer: &Layer, row: usize) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    if let Some(trits) = layer.row(row) {
        for trit in trits {
            hash = fnv(hash, &[trit.to_i8() as u8]);
        }
    }
    if let Some(bias) = layer.bias_trit(row) {
        hash = fnv(hash, &[bias.to_i8() as u8]);
    }
    if let Some(scale) = layer.scale(row) {
        hash = fnv(hash, &scale.to_bits().to_le_bytes());
    }
    format!("{hash:016x}")
}

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
