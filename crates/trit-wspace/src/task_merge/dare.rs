//! DARE 路径的任务合并（从 `task_merge.rs` 拆出）：先投票再稀疏化，并返回投票阶段报告。
//!
//! `*_report` 变体让 CLI/MCP 在 `--dare-rate` / `--dare-per-task` 分支也能输出
//! `merge_report`（此前 DARE 路径固定为 `null`）。

use super::{MergeReport, merge_tasks_weighted_report, validate_weights};
use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::task::TaskVector;

/// 把 f32 偏置整层复制到另一个同架构网络（DARE 只改三值差分，幅度另行保留）。
fn copy_bias_f32(from: &Net, to: &mut Net) -> Result<()> {
    for (index, source) in from.layers().iter().enumerate() {
        let target = to
            .layer(index)
            .ok_or(Error::OutOfBounds {
                layer: index,
                row: 0,
                col: 0,
            })?
            .clone();
        *to.layer_mut(index) = target.with_bias_f32(source.bias_f32().to_vec())?;
    }
    Ok(())
}

/// 投票结果 → DARE 稀疏化；`rate = 0` 原样返回。
fn sparse_after_vote(base: &Net, combined: &Net, rate: f32, seed: u64) -> Result<Net> {
    if rate == 0.0 {
        return Ok(combined.clone());
    }
    let outcome = crate::dare::dare_drop(base, combined, rate, seed)?;
    let mut result = base.clone();
    outcome.patch.apply(&mut result)?;
    copy_bias_f32(combined, &mut result)?;
    Ok(result)
}

/// 加权 DARE 合并（先投票再稀疏化），同时返回**投票阶段**报告。
pub fn merge_tasks_weighted_dare_report(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
    rate: f32,
    seed: u64,
) -> Result<(Net, MergeReport)> {
    let (combined, report) = merge_tasks_weighted_report(base, tasks, weights)?;
    let net = sparse_after_vote(base, &combined, rate, seed)?;
    Ok((net, report))
}

/// 加权 DARE 合并（先投票再稀疏化）；报告见 [`merge_tasks_weighted_dare_report`]。
pub fn merge_tasks_weighted_dare(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
    rate: f32,
    seed: u64,
) -> Result<Net> {
    Ok(merge_tasks_weighted_dare_report(base, tasks, weights, rate, seed)?.0)
}

/// 等权 DARE 合并（先投票再稀疏化），同时返回**投票阶段**报告。
pub fn merge_tasks_dare_report(
    base: &Net,
    tasks: &[TaskVector],
    rate: f32,
    seed: u64,
) -> Result<(Net, MergeReport)> {
    let weights = vec![1.0; tasks.len()];
    merge_tasks_weighted_dare_report(base, tasks, &weights, rate, seed)
}

/// 等权 DARE 合并（先投票再稀疏化）；报告见 [`merge_tasks_dare_report`]。
pub fn merge_tasks_dare(base: &Net, tasks: &[TaskVector], rate: f32, seed: u64) -> Result<Net> {
    Ok(merge_tasks_dare_report(base, tasks, rate, seed)?.0)
}

/// 每个任务先做坐标级 DARE，再按权重投票合并；返回**投票阶段**报告。
///
/// 丢弃发生在投票之前：被丢的票不参与加权投票。合并后对有差异的行统一乘一次
/// `1/(1-rate)`（与 [`merge_tasks_per_task_dare`] 相同的近似重标度）。
pub fn merge_tasks_weighted_per_task_dare_report(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
    rate: f32,
    seed: u64,
) -> Result<(Net, MergeReport)> {
    validate_weights(tasks.len(), weights)?;
    if !rate.is_finite() || !(0.0..1.0).contains(&rate) {
        return Err(Error::InvalidRate { rate });
    }
    if rate == 0.0 {
        return merge_tasks_weighted_report(base, tasks, weights);
    }
    let scale_factor = 1.0 / (1.0 - rate);
    let mut sparse = Vec::with_capacity(tasks.len());
    for (index, task) in tasks.iter().enumerate() {
        let child_seed = seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let edits: Vec<Edit> = task
            .edits()
            .iter()
            .filter(|edit| match edit {
                Edit::SetTrit {
                    layer, row, col, ..
                } => crate::dare::keep(child_seed, *layer, *row, *col, rate),
                Edit::SetBias { layer, row, .. } => {
                    crate::dare::keep(child_seed, *layer, *row, usize::MAX, rate)
                }
                _ => true,
            })
            .cloned()
            .collect();
        sparse.push(TaskVector {
            base: task.base,
            edits,
            scales: task.scales().to_owned(),
            bias_f32: task.bias_f32().to_vec(),
        });
    }
    let (mut net, report) = merge_tasks_weighted_report(base, &sparse, weights)?;
    let mut rescale = Vec::new();
    for (layer, (src, dst)) in base.layers().iter().zip(net.layers()).enumerate() {
        for row in 0..src.rows() {
            let changed = (0..src.cols()).any(|col| src.trit(row, col) != dst.trit(row, col))
                || src.bias_trit(row) != dst.bias_trit(row);
            if !changed {
                continue;
            }
            let scale = dst.scale(row).expect("row in range");
            let scaled = scale * scale_factor;
            if scaled.to_bits() != scale.to_bits() {
                rescale.push(Edit::SetScale {
                    layer,
                    row,
                    to: scaled,
                });
            }
        }
    }
    Patch::new(rescale).apply(&mut net)?;
    Ok((net, report))
}

/// 每个任务先做坐标级 DARE，再按权重投票合并；报告见 `_report` 变体。
pub fn merge_tasks_weighted_per_task_dare(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
    rate: f32,
    seed: u64,
) -> Result<Net> {
    Ok(merge_tasks_weighted_per_task_dare_report(base, tasks, weights, rate, seed)?.0)
}

/// 每个任务先做坐标级 DARE 再等权投票，同时返回**投票阶段**报告。
pub fn merge_tasks_per_task_dare_report(
    base: &Net,
    tasks: &[TaskVector],
    rate: f32,
    seed: u64,
) -> Result<(Net, MergeReport)> {
    let weights = vec![1.0; tasks.len()];
    merge_tasks_weighted_per_task_dare_report(base, tasks, &weights, rate, seed)
}

/// 每个任务先做坐标级 DARE 再等权投票；报告见 [`merge_tasks_per_task_dare_report`]。
pub fn merge_tasks_per_task_dare(
    base: &Net,
    tasks: &[TaskVector],
    rate: f32,
    seed: u64,
) -> Result<Net> {
    Ok(merge_tasks_per_task_dare_report(base, tasks, rate, seed)?.0)
}
