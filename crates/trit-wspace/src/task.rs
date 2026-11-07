//! 三值任务向量：`base → tuned` 的逐 trit 差分，多任务按目标值投票合并。
//!
//! 与 TIES 的对应关系：每个任务只对**它改过的坐标**投票，投票值是目标 trit；
//! 票和为正/负取该符号，出现平票（如 `+` 与 `-`）保留基准值；全体都投 `0` 时取 `0`。
//! scale 是正标尺自由度，按**行**单独记录差分；合并时对同一行取各任务目标值的
//! 中位数（偶数个取中间两个的平均），与基准位相同则不写编辑。

use std::collections::BTreeMap;

use crate::codec::checksum;
use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::trit::Trit;

/// 相对某个基准网络的任务向量。
#[derive(Clone, Debug, PartialEq)]
pub struct TaskVector {
    base: u64,
    edits: Vec<Edit>,
    /// 与基准不同的逐行 scale：`(layer, row, target)`。
    scales: Vec<(usize, usize, f32)>,
}

impl TaskVector {
    /// 从 `base → tuned` 求任务向量（只记录 trit/偏置，忽略 scale）。
    pub fn between(base: &Net, tuned: &Net) -> Result<Self> {
        if base.arch() != tuned.arch() {
            return Err(Error::ArchMismatch);
        }
        let diff = Patch::diff(base, tuned)?;
        let mut edits = Vec::new();
        let mut scales = Vec::new();
        for edit in diff.edits() {
            match edit {
                Edit::SetTrit { .. } | Edit::SetBias { .. } => edits.push(edit.clone()),
                Edit::SetScale { layer, row, to } => scales.push((*layer, *row, *to)),
                _ => {}
            }
        }
        Ok(Self {
            base: checksum(base),
            edits,
            scales,
        })
    }

    /// 先消隐藏单元置换再求差分（`merge::align`，用于独立训练/重排过的网络）。
    pub fn between_aligned(base: &Net, tuned: &Net) -> Result<Self> {
        if base.arch() != tuned.arch() {
            return Err(Error::ArchMismatch);
        }
        let aligned = crate::merge::align(base, tuned)?;
        Self::between(base, &aligned)
    }

    /// 基准网络的校验和（防止拿错基准套用）。
    #[must_use]
    pub fn base(&self) -> u64 {
        self.base
    }

    /// 差分编辑（trit / 偏置）。
    #[must_use]
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    /// 逐行 scale 差分：`(layer, row, target)`。
    #[must_use]
    pub fn scales(&self) -> &[(usize, usize, f32)] {
        &self.scales
    }

    /// 把本任务向量应用到 `base`（基准校验和必须一致）。
    pub fn apply(&self, base: &Net) -> Result<Net> {
        if checksum(base) != self.base {
            return Err(Error::Audit("任务向量的基准工件不一致".into()));
        }
        let mut net = base.clone();
        Patch::new(self.edits.clone()).apply(&mut net)?;
        let scale_edits: Vec<Edit> = self
            .scales
            .iter()
            .map(|(layer, row, to)| Edit::SetScale {
                layer: *layer,
                row: *row,
                to: *to,
            })
            .collect();
        Patch::new(scale_edits).apply(&mut net)?;
        Ok(net)
    }
}

/// 多任务合并：按坐标对目标 trit 投票（只算改过该坐标的任务）。
pub fn merge_tasks(base: &Net, tasks: &[TaskVector]) -> Result<Net> {
    let expected = checksum(base);
    if tasks.iter().any(|task| task.base != expected) {
        return Err(Error::Audit("任务向量的基准工件不一致".into()));
    }
    let mut votes: BTreeMap<(usize, usize, usize), Vec<Trit>> = BTreeMap::new();
    for task in tasks {
        for edit in task.edits() {
            match edit {
                Edit::SetTrit {
                    layer,
                    row,
                    col,
                    to,
                } => votes.entry((*layer, *row, *col)).or_default().push(*to),
                Edit::SetBias { layer, row, to } => votes
                    .entry((*layer, *row, usize::MAX))
                    .or_default()
                    .push(*to),
                _ => {}
            }
        }
    }
    let mut edits = Vec::new();
    for ((layer, row, col), targets) in votes {
        let current = current_value(base, layer, row, col)?;
        let sum: i32 = targets.iter().map(|target| i32::from(target.to_i8())).sum();
        let merged = if sum > 0 {
            Trit::Pos
        } else if sum < 0 {
            Trit::Neg
        } else if targets.iter().all(|target| *target == targets[0]) {
            targets[0]
        } else {
            current
        };
        if merged != current {
            edits.push(if col == usize::MAX {
                Edit::SetBias {
                    layer,
                    row,
                    to: merged,
                }
            } else {
                Edit::SetTrit {
                    layer,
                    row,
                    col,
                    to: merged,
                }
            });
        }
    }
    let mut scale_votes: BTreeMap<(usize, usize), Vec<f32>> = BTreeMap::new();
    for task in tasks {
        for (layer, row, value) in task.scales() {
            scale_votes.entry((*layer, *row)).or_default().push(*value);
        }
    }
    let mut scale_edits = Vec::new();
    for ((layer, row), mut values) in scale_votes {
        values.sort_by(f32::total_cmp);
        let merged = if values.len() % 2 == 1 {
            values[values.len() / 2]
        } else {
            let mid = values.len() / 2;
            (values[mid - 1] + values[mid]) * 0.5
        };
        let current = base
            .layer(layer)
            .and_then(|layer| layer.scale(row))
            .ok_or(Error::OutOfBounds { layer, row, col: 0 })?;
        if merged.to_bits() != current.to_bits() {
            scale_edits.push(Edit::SetScale {
                layer,
                row,
                to: merged,
            });
        }
    }
    let mut net = base.clone();
    Patch::new(edits).apply(&mut net)?;
    Patch::new(scale_edits).apply(&mut net)?;
    Ok(net)
}

/// 先按投票合并，再对 `base → merged` 的差分做一次**坐标级** DARE 稀疏化。
///
/// 这是“合并结果稀疏化”，不是“每个任务先稀疏化再投票”：投票阶段仍使用完整任务向量，
/// 避免丢弃改变投票结果；`rate=0` 与 [`merge_tasks`] 完全一致。`seed` 只影响 DARE 选点。
pub fn merge_tasks_dare(base: &Net, tasks: &[TaskVector], rate: f32, seed: u64) -> Result<Net> {
    let merged = merge_tasks(base, tasks)?;
    if rate == 0.0 {
        return Ok(merged);
    }
    let outcome = crate::dare::dare_drop(base, &merged, rate, seed)?;
    let mut net = base.clone();
    outcome.patch.apply(&mut net)?;
    Ok(net)
}

/// 每个任务**先**做坐标级 DARE 再投票（与 [`merge_tasks_dare`] 的“先合并再稀疏化”相对）。
///
/// 丢弃发生在投票之前：被丢的坐标不参与投票，可能改变投票结果。每个任务用
/// `seed ^ index * 常数` 派生子种子，保证多任务之间选择集独立；合并后对有差异的行
/// 统一乘一次 `1/(1-rate)`（近似 DARE 重标度，不是每任务各乘一次）。`rate=0` 与
/// [`merge_tasks`] 完全一致。
pub fn merge_tasks_per_task_dare(
    base: &Net,
    tasks: &[TaskVector],
    rate: f32,
    seed: u64,
) -> Result<Net> {
    if !rate.is_finite() || !(0.0..1.0).contains(&rate) {
        return Err(Error::InvalidRate { rate });
    }
    if rate == 0.0 {
        return merge_tasks(base, tasks);
    }
    let factor = 1.0 / (1.0 - rate);
    let mut sparse = Vec::with_capacity(tasks.len());
    for (index, task) in tasks.iter().enumerate() {
        let task_seed = seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let edits: Vec<Edit> = task
            .edits()
            .iter()
            .filter(|edit| match edit {
                Edit::SetTrit {
                    layer, row, col, ..
                } => crate::dare::keep(task_seed, *layer, *row, *col, rate),
                Edit::SetBias { layer, row, .. } => {
                    crate::dare::keep(task_seed, *layer, *row, usize::MAX, rate)
                }
                _ => true,
            })
            .cloned()
            .collect();
        sparse.push(TaskVector {
            base: task.base,
            edits,
            scales: task.scales().to_vec(),
        });
    }
    let mut net = merge_tasks(base, &sparse)?;
    let mut rescale = Vec::new();
    for (layer, (src, dst)) in base.layers().iter().zip(net.layers()).enumerate() {
        for row in 0..src.rows() {
            let changed = (0..src.cols()).any(|col| src.trit(row, col) != dst.trit(row, col))
                || src.bias_trit(row) != dst.bias_trit(row);
            if !changed {
                continue;
            }
            let scale = dst.scale(row).expect("row in range");
            let rescaled = scale * factor;
            if rescaled.to_bits() != scale.to_bits() {
                rescale.push(Edit::SetScale {
                    layer,
                    row,
                    to: rescaled,
                });
            }
        }
    }
    Patch::new(rescale).apply(&mut net)?;
    Ok(net)
}

fn current_value(base: &Net, layer: usize, row: usize, col: usize) -> Result<Trit> {
    let target = base
        .layer(layer)
        .ok_or(Error::OutOfBounds { layer, row, col: 0 })?;
    if row >= target.rows() {
        return Err(Error::OutOfBounds { layer, row, col: 0 });
    }
    let value = if col == usize::MAX {
        target.bias_trit(row)
    } else if col < target.cols() {
        target.trit(row, col)
    } else {
        None
    };
    value.ok_or(Error::OutOfBounds {
        layer,
        row,
        col: col.min(target.cols()),
    })
}
