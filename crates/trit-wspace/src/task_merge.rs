//! 多任务合并（从 `task.rs` 拆出，控制单文件行数）。
//!
//! 投票/加权中位数/DARE 与 f32 偏置合并；`TaskVector` 本体在 [`mod@crate::task`]。

use std::collections::BTreeMap;

use crate::codec::checksum;
use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::task::TaskVector;
use crate::trit::Trit;

/// 按差分写回逐行 f32 偏置：`None` 表示该行没有；层内任一行缺失 ⇒ 整层清空。
pub(crate) fn apply_bias_f32(net: &mut Net, records: &[(usize, usize, Option<f32>)]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut by_layer: BTreeMap<usize, BTreeMap<usize, Option<f32>>> = BTreeMap::new();
    for (layer, row, value) in records {
        by_layer.entry(*layer).or_default().insert(*row, *value);
    }
    for (layer, rows) in by_layer {
        let current = net
            .layer(layer)
            .ok_or(Error::OutOfBounds {
                layer,
                row: 0,
                col: 0,
            })?
            .clone();
        let mut values = Vec::with_capacity(current.rows());
        let mut complete = true;
        for row in 0..current.rows() {
            let value = match rows.get(&row) {
                Some(value) => *value,
                None => current.bias_f32().get(row).copied(),
            };
            match value {
                Some(value) => values.push(value),
                None => {
                    complete = false;
                    break;
                }
            }
        }
        if !complete {
            values.clear();
        }
        *net.layer_mut(layer) = current.with_bias_f32(values)?;
    }
    Ok(())
}

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

/// 多任务 f32 偏置：权重大于 0 的任务全部有幅度才取加权平均，任一缺失 ⇒ 该行清空。
fn merge_bias_f32(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
) -> Result<Vec<(usize, usize, Option<f32>)>> {
    let mut merged = Vec::new();
    for (layer, target) in base.layers().iter().enumerate() {
        for row in 0..target.rows() {
            let mut seen = false;
            let mut missing = false;
            let mut weighted = 0.0f64;
            let mut total = 0.0f64;
            for (task, weight) in tasks.iter().zip(weights) {
                if *weight == 0.0 {
                    continue;
                }
                seen = true;
                let state = task
                    .bias_f32
                    .iter()
                    .find(|(l, r, _)| *l == layer && *r == row)
                    .map(|(_, _, value)| *value)
                    .unwrap_or_else(|| target.bias_f32().get(row).copied());
                match state {
                    Some(value) => {
                        weighted += f64::from(value) * f64::from(*weight);
                        total += f64::from(*weight);
                    }
                    None => {
                        missing = true;
                        break;
                    }
                }
            }
            if !seen {
                continue;
            }
            let current = target.bias_f32().get(row).copied();
            let next = if missing {
                None
            } else {
                Some((weighted / total) as f32)
            };
            if current.map(f32::to_bits) != next.map(f32::to_bits) {
                merged.push((layer, row, next));
            }
        }
    }
    Ok(merged)
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
    let bias_f32 = merge_bias_f32(base, tasks, &vec![1.0; tasks.len()])?;
    apply_bias_f32(&mut net, &bias_f32)?;
    Ok(net)
}

/// 多任务加权合并：每个任务带一个权重，票和按权重累加。
///
/// `weights` 必须与 `tasks` 等长，每个权重是有限非负数；`0.0` 表示忽略该任务
/// （它的 trit/偏置票不进投票，scale 也不进统计）。权重全为 1 时与
/// [`merge_tasks`] 逐位一致。scale 用**加权中位数**：排序后取累计权重达到总权重
/// 一半处的值，恰好落在两个值之间时取平均（等权时退化为普通中位数）。
/// 合并报告：投票/平票/忽略/改动/scale 行数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeReport {
    tasks: usize,
    ignored_tasks: usize,
    voted: usize,
    ties: usize,
    changed: usize,
    scale_rows: usize,
}

impl MergeReport {
    /// 参与合并的任务数。
    #[must_use]
    pub fn tasks(&self) -> usize {
        self.tasks
    }

    /// 权重为 0、完全不参与的任务数。
    #[must_use]
    pub fn ignored_tasks(&self) -> usize {
        self.ignored_tasks
    }

    /// 至少收到一票的坐标数。
    #[must_use]
    pub fn voted(&self) -> usize {
        self.voted
    }

    /// 平票（票和为零且票值不全相同）而保留基准的坐标数。
    #[must_use]
    pub fn ties(&self) -> usize {
        self.ties
    }

    /// 实际写出的 trit/偏置编辑数。
    #[must_use]
    pub fn changed(&self) -> usize {
        self.changed
    }

    /// 实际写出的 scale 行数。
    #[must_use]
    pub fn scale_rows(&self) -> usize {
        self.scale_rows
    }
}

/// 多任务加权合并：返回网络（不带报告）。
pub fn merge_tasks_weighted(base: &Net, tasks: &[TaskVector], weights: &[f32]) -> Result<Net> {
    validate_weights(tasks.len(), weights)?;
    merge_weighted(base, tasks, weights).map(|(net, _)| net)
}

/// 多任务加权合并：同时返回投票/平票/忽略等统计。
pub fn merge_tasks_weighted_report(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
) -> Result<(Net, MergeReport)> {
    validate_weights(tasks.len(), weights)?;
    merge_weighted(base, tasks, weights)
}

fn validate_weights(task_count: usize, weights: &[f32]) -> Result<()> {
    if weights.len() != task_count {
        return Err(Error::Shape {
            what: "weights",
            expected: task_count,
            got: weights.len(),
        });
    }
    for (index, weight) in weights.iter().enumerate() {
        if !weight.is_finite() || *weight < 0.0 {
            return Err(Error::InvalidWeight {
                index,
                weight: *weight,
            });
        }
    }
    Ok(())
}

fn merge_weighted(base: &Net, tasks: &[TaskVector], weights: &[f32]) -> Result<(Net, MergeReport)> {
    let base_checksum = checksum(base);
    if tasks.iter().any(|task| task.base != base_checksum) {
        return Err(Error::Audit("任务向量的基准工件不一致".into()));
    }
    let ignored_tasks = weights.iter().filter(|weight| **weight == 0.0).count();
    let mut votes: BTreeMap<(usize, usize, usize), Vec<(Trit, f64)>> = BTreeMap::new();
    for (task, weight) in tasks.iter().zip(weights) {
        let weighted = f64::from(*weight);
        if weighted == 0.0 {
            continue;
        }
        for edit in task.edits() {
            match edit {
                Edit::SetTrit {
                    layer,
                    row,
                    col,
                    to,
                } => votes
                    .entry((*layer, *row, *col))
                    .or_default()
                    .push((*to, weighted)),
                Edit::SetBias { layer, row, to } => votes
                    .entry((*layer, *row, usize::MAX))
                    .or_default()
                    .push((*to, weighted)),
                _ => {}
            }
        }
    }
    let voted = votes.len();
    let mut ties = 0usize;
    let mut weighted_edits = Vec::new();
    for ((layer, row, col), targets) in votes {
        let current = current_value(base, layer, row, col)?;
        let sum: f64 = targets
            .iter()
            .map(|(target, weight)| f64::from(target.to_i8()) * weight)
            .sum();
        let merged = if sum > 0.0 {
            Trit::Pos
        } else if sum < 0.0 {
            Trit::Neg
        } else if targets.iter().all(|(target, _)| *target == targets[0].0) {
            targets[0].0
        } else {
            ties += 1;
            current
        };
        if merged != current {
            weighted_edits.push(if col == usize::MAX {
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
    let changed = weighted_edits.len();
    let mut scale_votes: BTreeMap<(usize, usize), Vec<(f32, f64)>> = BTreeMap::new();
    for (task, weight) in tasks.iter().zip(weights) {
        if *weight == 0.0 {
            continue;
        }
        for (layer, row, value) in task.scales() {
            scale_votes
                .entry((*layer, *row))
                .or_default()
                .push((*value, f64::from(*weight)));
        }
    }
    let mut scales = Vec::new();
    for ((layer, row), items) in scale_votes {
        let Some(merged_scale) = weighted_median(items) else {
            continue;
        };
        let current = base
            .layer(layer)
            .and_then(|layer| layer.scale(row))
            .ok_or(Error::OutOfBounds { layer, row, col: 0 })?;
        if merged_scale.to_bits() != current.to_bits() {
            scales.push(Edit::SetScale {
                layer,
                row,
                to: merged_scale,
            });
        }
    }
    let scale_rows = scales.len();
    let mut net = base.clone();
    Patch::new(weighted_edits).apply(&mut net)?;
    Patch::new(scales).apply(&mut net)?;
    let bias_f32 = merge_bias_f32(base, tasks, weights)?;
    apply_bias_f32(&mut net, &bias_f32)?;
    Ok((
        net,
        MergeReport {
            tasks: tasks.len(),
            ignored_tasks,
            voted,
            ties,
            changed,
            scale_rows,
        },
    ))
}

fn weighted_median(mut items: Vec<(f32, f64)>) -> Option<f32> {
    items.retain(|(_, weight)| *weight > 0.0);
    if items.is_empty() {
        return None;
    }
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = items.iter().map(|(_, weight)| *weight).sum();
    let half = total * 0.5;
    let mut cumulative = 0.0;
    for (index, (value, weight)) in items.iter().enumerate() {
        let next = cumulative + *weight;
        if next > half {
            return Some(*value);
        }
        if next == half {
            if let Some((next_value, _)) = items.get(index + 1) {
                return Some((*value + *next_value) * 0.5);
            }
            return Some(*value);
        }
        cumulative = next;
    }
    items.last().map(|(value, _)| *value)
}

/// 先按加权投票合并，再对 `base → merged` 的差分做一次坐标级 DARE。
///
/// `rate=0` 与 [`merge_tasks_weighted`] 完全一致；权重校验同前者。
pub fn merge_tasks_weighted_dare(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
    rate: f32,
    seed: u64,
) -> Result<Net> {
    let combined = merge_tasks_weighted(base, tasks, weights)?;
    if rate == 0.0 {
        return Ok(combined);
    }
    let outcome = crate::dare::dare_drop(base, &combined, rate, seed)?;
    let mut result = base.clone();
    outcome.patch.apply(&mut result)?;
    copy_bias_f32(&combined, &mut result)?;
    Ok(result)
}

/// 每个任务先做坐标级 DARE，再按权重投票合并。
///
/// 丢弃发生在投票之前：被丢的票不参与加权投票。合并后对有差异的行统一乘一次
/// `1/(1-rate)`（与 [`merge_tasks_per_task_dare`] 相同的近似重标度）。
pub fn merge_tasks_weighted_per_task_dare(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
    rate: f32,
    seed: u64,
) -> Result<Net> {
    validate_weights(tasks.len(), weights)?;
    if !rate.is_finite() || !(0.0..1.0).contains(&rate) {
        return Err(Error::InvalidRate { rate });
    }
    if rate == 0.0 {
        return merge_tasks_weighted(base, tasks, weights);
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
    let mut net = merge_tasks_weighted(base, &sparse, weights)?;
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
    copy_bias_f32(&merged, &mut net)?;
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
            bias_f32: task.bias_f32().to_vec(),
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
