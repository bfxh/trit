//! 三值任务向量：`base → tuned` 的逐 trit 差分，多任务按目标值投票合并。
//!
//! 与 TIES 的对应关系：每个任务只对**它改过的坐标**投票，投票值是目标 trit；
//! 票和为正/负取该符号，出现平票（如 `+` 与 `-`）保留基准值；全体都投 `0` 时取 `0`。
//! scale 是正标尺自由度，不进任务向量（合并后沿用基准的 scale）。

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
}

impl TaskVector {
    /// 从 `base → tuned` 求任务向量（只记录 trit/偏置，忽略 scale）。
    pub fn between(base: &Net, tuned: &Net) -> Result<Self> {
        if base.arch() != tuned.arch() {
            return Err(Error::ArchMismatch);
        }
        let edits: Vec<Edit> = Patch::diff(base, tuned)?
            .edits()
            .iter()
            .filter(|edit| matches!(edit, Edit::SetTrit { .. } | Edit::SetBias { .. }))
            .cloned()
            .collect();
        Ok(Self {
            base: checksum(base),
            edits,
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

    /// 差分编辑。
    #[must_use]
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    /// 把本任务向量应用到 `base`（基准校验和必须一致）。
    pub fn apply(&self, base: &Net) -> Result<Net> {
        if checksum(base) != self.base {
            return Err(Error::Audit("任务向量的基准工件不一致".into()));
        }
        let mut net = base.clone();
        Patch::new(self.edits.clone()).apply(&mut net)?;
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
    let mut net = base.clone();
    Patch::new(edits).apply(&mut net)?;
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
