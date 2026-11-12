//! 任务向量缩放（从 `task.rs` 拆出，控制单文件行数）。
//!
//! 三值任务算术：全局 λ、按层、按行、按列块；`0` 丢弃、`<0` 取反、`>0` 原样。
//!
//! f32 偏置只在所有 λ 都 `> 0` 时原样保留；任一 `<= 0` 就丢弃该任务的 f32 偏置
//! （幅度无法与取反/丢弃后的三值语义一一对应，宁可退回三值符号）。

use std::collections::BTreeMap;

use crate::edit::Edit;
use crate::error::{Error, Result};
use crate::net::Net;
use crate::task::TaskVector;
use crate::task_merge::merge_tasks_weighted;
use crate::trit::Trit;

/// 三值任务算术：`lambda > 0` 原样、`lambda == 0` 丢弃、`lambda < 0` 取反。
///
/// 三值权重没有幅度，正系数不改变符号；因此只有符号/丢弃两种结果。`lambda < 0` 时
/// trit 与偏置取反，**scale 差分丢弃**（负 scale 非法，而正的倍率在取反后无法保持语义）。
pub fn scale_task(vector: &TaskVector, lambda: f32) -> Result<TaskVector> {
    if !lambda.is_finite() {
        return Err(Error::InvalidLambda { lambda });
    }
    if lambda == 0.0 {
        return Ok(TaskVector::empty(vector));
    }
    if lambda > 0.0 {
        return Ok(vector.clone());
    }
    let edits = vector
        .edits()
        .iter()
        .map(|edit| match edit {
            Edit::SetTrit {
                layer,
                row,
                col,
                to,
            } => Edit::SetTrit {
                layer: *layer,
                row: *row,
                col: *col,
                to: negate_trit(*to),
            },
            Edit::SetBias { layer, row, to } => Edit::SetBias {
                layer: *layer,
                row: *row,
                to: negate_trit(*to),
            },
            other => other.clone(),
        })
        .collect();
    Ok(TaskVector::rebuild(vector, edits, Vec::new(), false))
}

/// 缩放单位：层 / 行 / 列块。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleUnit {
    /// 每层一个系数。
    Layer,
    /// 每行一个系数（按层顺序扁平编号）。
    Row,
    /// 每个列块一个系数（`col / block`；每行 `ceil(cols / block)` 块）。
    Block(usize),
}

/// 按单位缩放：`lambdas` 的长度由单位决定（层数 / 总行数 / 总块数）。
///
/// `dims` 是每层的 `(rows, cols)`，用于把 `(layer, row, col)` 映射到扁平索引；
/// `ScaleUnit::Layer` 不需要 `dims`。`0` 丢弃该单位、`<0` 取反 trit/偏置、`>0` 原样；
/// `λ <= 0` 的 scale 差分丢弃。
pub fn scale_task_units(
    vector: &TaskVector,
    lambdas: &[f32],
    dims: &[(usize, usize)],
    unit: ScaleUnit,
) -> Result<TaskVector> {
    for lambda in lambdas {
        if !lambda.is_finite() {
            return Err(Error::InvalidLambda { lambda: *lambda });
        }
    }
    if matches!(unit, ScaleUnit::Block(0)) {
        return Err(Error::InvalidBlock { block: 0 });
    }
    let index_of = |layer: usize, row: usize, col: usize| -> Result<usize> {
        match unit {
            ScaleUnit::Layer => Ok(layer),
            ScaleUnit::Row => {
                let mut offset = 0usize;
                for (rows, _) in dims.iter().take(layer) {
                    offset += rows;
                }
                if dims.get(layer).is_none() {
                    return Err(Error::Shape {
                        what: "row_lambdas",
                        expected: layer + 1,
                        got: dims.len(),
                    });
                }
                Ok(offset + row)
            }
            ScaleUnit::Block(block) => {
                let mut offset = 0usize;
                for (rows, cols) in dims.iter().take(layer) {
                    offset += rows * cols.div_ceil(block);
                }
                let (_, cols) = dims.get(layer).ok_or(Error::Shape {
                    what: "block_lambdas",
                    expected: layer + 1,
                    got: dims.len(),
                })?;
                Ok(offset + row * cols.div_ceil(block) + col / block)
            }
        }
    };
    let keep_bias = lambdas.iter().all(|lambda| *lambda > 0.0);
    let mut edits = Vec::new();
    for edit in vector.edits() {
        let (layer, row, col) = match edit {
            Edit::SetTrit {
                layer, row, col, ..
            } => (*layer, *row, *col),
            Edit::SetBias { layer, row, .. } => (*layer, *row, 0),
            _ => continue,
        };
        let index = index_of(layer, row, col)?;
        let lambda = *lambdas.get(index).ok_or(Error::Shape {
            what: "unit_lambdas",
            expected: index + 1,
            got: lambdas.len(),
        })?;
        if lambda == 0.0 {
            continue;
        }
        let edit = if lambda < 0.0 {
            match edit {
                Edit::SetTrit {
                    layer,
                    row,
                    col,
                    to,
                } => Edit::SetTrit {
                    layer: *layer,
                    row: *row,
                    col: *col,
                    to: negate_trit(*to),
                },
                Edit::SetBias { layer, row, to } => Edit::SetBias {
                    layer: *layer,
                    row: *row,
                    to: negate_trit(*to),
                },
                other => other.clone(),
            }
        } else {
            edit.clone()
        };
        edits.push(edit);
    }
    let mut scales = Vec::new();
    for (layer, row, scale) in vector.scales() {
        let index = index_of(*layer, *row, 0)?;
        let lambda = *lambdas.get(index).ok_or(Error::Shape {
            what: "unit_lambdas",
            expected: index + 1,
            got: lambdas.len(),
        })?;
        if lambda > 0.0 {
            scales.push((*layer, *row, *scale));
        }
    }
    Ok(TaskVector::rebuild(vector, edits, scales, keep_bias))
}

/// 按行缩放：`lambdas` 按每层行数扁平编号（`dims` 的 `rows` 依次累加）。
pub fn scale_task_rows(
    vector: &TaskVector,
    lambdas: &[f32],
    dims: &[(usize, usize)],
) -> Result<TaskVector> {
    scale_task_units(vector, lambdas, dims, ScaleUnit::Row)
}

/// 按列块缩放：`lambdas` 按每层 `rows * ceil(cols / block)` 扁平编号。
pub fn scale_task_blocks(
    vector: &TaskVector,
    lambdas: &[f32],
    dims: &[(usize, usize)],
    block: usize,
) -> Result<TaskVector> {
    scale_task_units(vector, lambdas, dims, ScaleUnit::Block(block))
}

/// 单位键的一段：`*`（任意）或数字集合（`1|3-5`）。
#[derive(Clone, Debug, PartialEq)]
pub enum UnitPart {
    /// `*`：任意值。
    Any,
    /// 数字/闭区间集合，如 `1`、`1-3`、`1|3-5`。
    Ranges(Vec<std::ops::RangeInclusive<usize>>),
}

impl UnitPart {
    fn matches(&self, value: &usize) -> bool {
        match self {
            UnitPart::Any => true,
            UnitPart::Ranges(ranges) => ranges.iter().any(|range| range.contains(value)),
        }
    }

    fn specificity(&self) -> usize {
        match self {
            UnitPart::Any => 0,
            UnitPart::Ranges(ranges)
                if ranges.len() == 1 && ranges[0].start() == ranges[0].end() =>
            {
                2
            }
            UnitPart::Ranges(_) => 1,
        }
    }
}

/// 单位键模式：每段是 [`UnitPart`]。
#[derive(Clone, Debug, PartialEq)]
pub struct UnitPattern {
    /// 键的每一段。
    pub parts: Vec<UnitPart>,
    /// 匹配时使用的 λ。
    pub lambda: f32,
}

impl UnitPattern {
    fn matches(&self, key: &[usize]) -> bool {
        if self.parts.len() != key.len() {
            return false;
        }
        self.parts
            .iter()
            .zip(key)
            .all(|(part, value)| part.matches(value))
    }

    fn specificity(&self) -> usize {
        self.parts.iter().map(UnitPart::specificity).sum()
    }
}

/// 按行模式缩放：键是 `[layer, row]`，支持 `*` 与 `a-b` 区间。
///
/// 匹配优先级：精确段 > 区间段 > 通配段（按具体度降序）；同具体度按传入顺序。
/// 未匹配的行默认 λ=1。
pub fn scale_task_rows_patterns(
    vector: &TaskVector,
    patterns: &[UnitPattern],
) -> Result<TaskVector> {
    validate_patterns(patterns)?;
    let mut ordered = patterns.to_vec();
    ordered.sort_by_key(|pattern| std::cmp::Reverse(pattern.specificity()));
    let keep_bias = patterns.iter().all(|pattern| pattern.lambda > 0.0);
    let mut edits = Vec::new();
    for edit in vector.edits() {
        let (layer, row, _) = edit_key(edit);
        let lambda = pattern_lambda(&ordered, &[layer, row]);
        if let Some(edit) = scaled_edit(edit, lambda) {
            edits.push(edit);
        }
    }
    let mut scales = Vec::new();
    for (layer, row, scale) in vector.scales() {
        let lambda = pattern_lambda(&ordered, &[*layer, *row]);
        if lambda > 0.0 {
            scales.push((*layer, *row, *scale));
        }
    }
    Ok(TaskVector::rebuild(vector, edits, scales, keep_bias))
}

/// 按块模式缩放：键是 `[layer, row, col/block]`，支持 `*` 与 `a-b` 区间。
pub fn scale_task_blocks_patterns(
    vector: &TaskVector,
    patterns: &[UnitPattern],
    block: usize,
) -> Result<TaskVector> {
    if block == 0 {
        return Err(Error::InvalidBlock { block });
    }
    validate_patterns(patterns)?;
    let mut ordered = patterns.to_vec();
    ordered.sort_by_key(|pattern| std::cmp::Reverse(pattern.specificity()));
    let keep_bias = patterns.iter().all(|pattern| pattern.lambda > 0.0);
    let mut edits = Vec::new();
    for edit in vector.edits() {
        let (layer, row, col) = edit_key(edit);
        let lambda = pattern_lambda(&ordered, &[layer, row, col / block]);
        if let Some(edit) = scaled_edit(edit, lambda) {
            edits.push(edit);
        }
    }
    let mut scales = Vec::new();
    for (layer, row, scale) in vector.scales() {
        let lambda = pattern_lambda(&ordered, &[*layer, *row, 0]);
        if lambda > 0.0 {
            scales.push((*layer, *row, *scale));
        }
    }
    Ok(TaskVector::rebuild(vector, edits, scales, keep_bias))
}

fn validate_patterns(patterns: &[UnitPattern]) -> Result<()> {
    for pattern in patterns {
        if !pattern.lambda.is_finite() {
            return Err(Error::InvalidLambda {
                lambda: pattern.lambda,
            });
        }
    }
    Ok(())
}

fn pattern_lambda(patterns: &[UnitPattern], key: &[usize]) -> f32 {
    patterns
        .iter()
        .find(|pattern| pattern.matches(key))
        .map_or(1.0, |pattern| pattern.lambda)
}

/// 按 `(layer, row)` 键值对缩放；未给出的行默认保留（λ=1）。
///
/// 键值对是**稀疏**形式：只写出需要改的行，其余行按 λ=1 处理。`λ=0` 丢弃该行、
/// `<0` 取反、`>0` 原样；`λ <= 0` 的 scale 差分丢弃。
pub fn scale_task_rows_map(
    vector: &TaskVector,
    lambdas: &BTreeMap<(usize, usize), f32>,
) -> Result<TaskVector> {
    validate_map(lambdas.values())?;
    let keep_bias = lambdas.values().all(|lambda| *lambda > 0.0);
    let mut edits = Vec::new();
    for edit in vector.edits() {
        let (layer, row, _) = edit_key(edit);
        let lambda = lambdas.get(&(layer, row)).copied().unwrap_or(1.0);
        if let Some(edit) = scaled_edit(edit, lambda) {
            edits.push(edit);
        }
    }
    let mut scales = Vec::new();
    for (layer, row, scale) in vector.scales() {
        let lambda = lambdas.get(&(*layer, *row)).copied().unwrap_or(1.0);
        if lambda > 0.0 {
            scales.push((*layer, *row, *scale));
        }
    }
    Ok(TaskVector::rebuild(vector, edits, scales, keep_bias))
}

/// 按 `(layer, row, block)` 键值对缩放；未给出的块默认保留（λ=1）。
///
/// 块索引是 `col / block`；`block` 必须大于 0。
pub fn scale_task_blocks_map(
    vector: &TaskVector,
    lambdas: &BTreeMap<(usize, usize, usize), f32>,
    block: usize,
) -> Result<TaskVector> {
    if block == 0 {
        return Err(Error::InvalidBlock { block });
    }
    validate_map(lambdas.values())?;
    let keep_bias = lambdas.values().all(|lambda| *lambda > 0.0);
    let mut edits = Vec::new();
    for edit in vector.edits() {
        let (layer, row, col) = edit_key(edit);
        let unit = col / block;
        let lambda = lambdas.get(&(layer, row, unit)).copied().unwrap_or(1.0);
        if let Some(edit) = scaled_edit(edit, lambda) {
            edits.push(edit);
        }
    }
    let mut scales = Vec::new();
    for (layer, row, scale) in vector.scales() {
        let lambda = lambdas.get(&(*layer, *row, 0)).copied().unwrap_or(1.0);
        if lambda > 0.0 {
            scales.push((*layer, *row, *scale));
        }
    }
    Ok(TaskVector::rebuild(vector, edits, scales, keep_bias))
}

fn validate_map<'a>(values: impl Iterator<Item = &'a f32>) -> Result<()> {
    for value in values {
        if !value.is_finite() {
            return Err(Error::InvalidLambda { lambda: *value });
        }
    }
    Ok(())
}

fn edit_key(edit: &Edit) -> (usize, usize, usize) {
    match edit {
        Edit::SetTrit {
            layer, row, col, ..
        } => (*layer, *row, *col),
        Edit::SetBias { layer, row, .. } => (*layer, *row, 0),
        _ => (0, 0, 0),
    }
}

fn scaled_edit(edit: &Edit, lambda: f32) -> Option<Edit> {
    if lambda == 0.0 {
        return None;
    }
    Some(if lambda < 0.0 {
        match edit {
            Edit::SetTrit {
                layer,
                row,
                col,
                to,
            } => Edit::SetTrit {
                layer: *layer,
                row: *row,
                col: *col,
                to: negate_trit(*to),
            },
            Edit::SetBias { layer, row, to } => Edit::SetBias {
                layer: *layer,
                row: *row,
                to: negate_trit(*to),
            },
            other => other.clone(),
        }
    } else {
        edit.clone()
    })
}

/// 三值任务算术组合：对每个任务按 `lambdas[i]` 缩放后加权合并。
///
/// `λ > 0` 保留、`λ == 0` 丢弃、`λ < 0` 取反；合并权重取 `|λ|`（三值没有幅度，
/// 绝对值只影响投票权重，不改变符号）。`tasks` 与 `lambdas` 必须等长。
pub fn combine_tasks(base: &Net, tasks: &[TaskVector], lambdas: &[f32]) -> Result<Net> {
    if tasks.len() != lambdas.len() {
        return Err(Error::Shape {
            what: "lambdas",
            expected: tasks.len(),
            got: lambdas.len(),
        });
    }
    let mut scaled = Vec::with_capacity(tasks.len());
    let mut weights = Vec::with_capacity(tasks.len());
    for (task, lambda) in tasks.iter().zip(lambdas) {
        if !lambda.is_finite() {
            return Err(Error::InvalidLambda { lambda: *lambda });
        }
        scaled.push(scale_task(task, *lambda)?);
        weights.push(lambda.abs());
    }
    merge_tasks_weighted(base, &scaled, &weights)
}

/// 按层缩放：`lambdas[layer]` 逐个作用在该层的差分上（`0` 丢弃、`<0` 取反、`>0` 原样）。
///
/// 与 [`scale_task`] 的全局系数不同，这里每个层可以给不同的 λ。`TaskVector` 只记录
/// 有差分的层，所以超出 `lambdas` 长度的层会判 `Shape`；CLI 会先用 `base.layers().len()`
/// 校验总长度。`λ <= 0` 的层其 scale 差分也丢弃（与 [`scale_task`] 的负系数语义一致）。
pub fn scale_task_layers(vector: &TaskVector, lambdas: &[f32]) -> Result<TaskVector> {
    for lambda in lambdas {
        if !lambda.is_finite() {
            return Err(Error::InvalidLambda { lambda: *lambda });
        }
    }
    let keep_bias = lambdas.iter().all(|lambda| *lambda > 0.0);
    let mut edits = Vec::new();
    for edit in vector.edits() {
        let layer = match edit {
            Edit::SetTrit { layer, .. } | Edit::SetBias { layer, .. } => *layer,
            _ => continue,
        };
        let lambda = *lambdas.get(layer).ok_or(Error::Shape {
            what: "layer_lambdas",
            expected: layer + 1,
            got: lambdas.len(),
        })?;
        if lambda == 0.0 {
            continue;
        }
        let edit = if lambda < 0.0 {
            match edit {
                Edit::SetTrit {
                    layer,
                    row,
                    col,
                    to,
                } => Edit::SetTrit {
                    layer: *layer,
                    row: *row,
                    col: *col,
                    to: negate_trit(*to),
                },
                Edit::SetBias { layer, row, to } => Edit::SetBias {
                    layer: *layer,
                    row: *row,
                    to: negate_trit(*to),
                },
                other => other.clone(),
            }
        } else {
            edit.clone()
        };
        edits.push(edit);
    }
    let mut scales = Vec::new();
    for (layer, row, scale) in vector.scales() {
        let lambda = *lambdas.get(*layer).ok_or(Error::Shape {
            what: "layer_lambdas",
            expected: layer + 1,
            got: lambdas.len(),
        })?;
        if lambda > 0.0 {
            scales.push((*layer, *row, *scale));
        }
    }
    Ok(TaskVector::rebuild(vector, edits, scales, keep_bias))
}

fn negate_trit(trit: Trit) -> Trit {
    match trit {
        Trit::Pos => Trit::Neg,
        Trit::Neg => Trit::Pos,
        Trit::Zero => Trit::Zero,
    }
}
