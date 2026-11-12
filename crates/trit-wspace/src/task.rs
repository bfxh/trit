//! 三值任务向量：`base → tuned` 的逐 trit 差分，多任务按目标值投票合并。
//!
//! 与 TIES 的对应关系：每个任务只对**它改过的坐标**投票，投票值是目标 trit；
//! 票和为正/负取该符号，出现平票（如 `+` 与 `-`）保留基准值；全体都投 `0` 时取 `0`。
//! scale 是正标尺自由度，按**行**单独记录差分；合并时对同一行取各任务目标值的
//! 中位数（偶数个取中间两个的平均），与基准位相同则不写编辑。加权合并
//! （[`crate::task_merge::merge_tasks_weighted`]）把每个任务的票乘上权重，scale 改用加权中位数；
//! 权重为 0 的任务完全不参与（票和 scale 统计都不进）。
//!
//! f32 偏置（只影响读数/导出）按行记录差分：合并时只有全部参与任务都有幅度才加权平均，
//! 任一任务缺该行就整层清空；DARE 丢弃三值差分后再贴回合并幅度。

use crate::codec::checksum;
use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;

/// 相对某个基准网络的任务向量。
#[derive(Clone, Debug, PartialEq)]
pub struct TaskVector {
    pub(crate) base: u64,
    pub(crate) edits: Vec<Edit>,
    /// 与基准不同的逐行 scale：`(layer, row, target)`。
    pub(crate) scales: Vec<(usize, usize, f32)>,
    /// 与基准不同的逐行 f32 偏置：`(layer, row, target)`，`None` 表示该行没有。
    ///
    /// 未记录的行表示与基准相同；`bias_f32` 只影响读数/导出，不参与三值行为。
    pub(crate) bias_f32: Vec<(usize, usize, Option<f32>)>,
}

impl TaskVector {
    /// 从 `base → tuned` 求任务向量（记录 trit/偏置/scale/f32 偏置差分）。
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
        let mut bias_f32 = Vec::new();
        for (layer, (from, to)) in base.layers().iter().zip(tuned.layers()).enumerate() {
            for row in 0..from.rows() {
                let left = from.bias_f32().get(row).copied();
                let right = to.bias_f32().get(row).copied();
                if left.map(f32::to_bits) != right.map(f32::to_bits) {
                    bias_f32.push((layer, row, right));
                }
            }
        }
        Ok(Self {
            base: checksum(base),
            edits,
            scales,
            bias_f32,
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

    /// 逐行 f32 偏置差分：`(layer, row, target)`；`None` 表示该行没有。
    #[must_use]
    pub fn bias_f32(&self) -> &[(usize, usize, Option<f32>)] {
        &self.bias_f32
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
        crate::task_merge::apply_bias_f32(&mut net, &self.bias_f32)?;
        Ok(net)
    }

    /// 用新的编辑/scale 重建向量；`positive` 为假时丢弃 f32 偏置（λ <= 0 的缩放语义）。
    pub(crate) fn rebuild(
        &self,
        edits: Vec<Edit>,
        scales: Vec<(usize, usize, f32)>,
        positive: bool,
    ) -> Self {
        Self {
            base: self.base,
            edits,
            scales,
            bias_f32: if positive {
                self.bias_f32.clone()
            } else {
                Vec::new()
            },
        }
    }

    /// 空向量（λ = 0：丢弃全部差分）。
    pub(crate) fn empty(&self) -> Self {
        Self {
            base: self.base,
            edits: Vec::new(),
            scales: Vec::new(),
            bias_f32: Vec::new(),
        }
    }
}
