//! 任务向量缩放（从 `task.rs` 拆出，控制单文件行数）。
//!
//! 三值任务算术：全局 λ、按层、按行、按列块；`0` 丢弃、`<0` 取反、`>0` 原样。

use crate::edit::Edit;
use crate::error::{Error, Result};
use crate::net::Net;
use crate::task::{TaskVector, merge_tasks_weighted};
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
        return Ok(TaskVector {
            base: vector.base,
            edits: Vec::new(),
            scales: Vec::new(),
        });
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
    Ok(TaskVector {
        base: vector.base,
        edits,
        scales: Vec::new(),
    })
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
    Ok(TaskVector {
        base: vector.base,
        edits,
        scales,
    })
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
    Ok(TaskVector {
        base: vector.base,
        edits,
        scales,
    })
}

fn negate_trit(trit: Trit) -> Trit {
    match trit {
        Trit::Pos => Trit::Neg,
        Trit::Neg => Trit::Pos,
        Trit::Zero => Trit::Zero,
    }
}
