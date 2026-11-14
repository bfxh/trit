//! 序列编辑的遗忘曲线：每一步之后，**全部历史 oracle** 的保持率。
//!
//! WISE / MEMIT 那类工作的核心问题不是「这次编辑有没有生效」，而是「连续编辑若干次后，
//! 早先的任务还剩多少」。三值权重让这件事可以精确算：保持率 = 正确样本数 / 样本数，
//! 没有容差；曲线就是「步骤 × 历史 oracle」的保持率矩阵。

use serde_json::{Value, json};

use crate::edit::Patch;
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;

/// 一步输入：要落地的补丁 + 本步自己的任务 oracle（可选）。
#[derive(Clone, Debug, PartialEq)]
pub struct ForgettingInput {
    /// 本步应用的补丁。
    pub patch: Patch,
    /// 本步自身任务 oracle；给了就报告该步的 loss。
    pub own: Option<Oracle>,
}

/// 一个步骤之后的读数。
#[derive(Clone, Debug, PartialEq)]
pub struct ForgettingStep {
    /// 步骤号（从 0 开始）。
    pub index: usize,
    /// 本步自己的任务 oracle 上的失配数（没给就是 `None`）。
    pub loss: Option<usize>,
    /// 对每个历史 oracle 的保持率（0..=1）。
    pub retention: Vec<f64>,
    /// 历史 oracle 的平均保持率（空历史为 1.0）。
    pub mean_retention: f64,
}

impl ForgettingStep {
    fn to_json(&self) -> Value {
        json!({
            "index": self.index,
            "loss": self.loss,
            "retention": self.retention,
            "mean_retention": self.mean_retention,
        })
    }
}

/// 整条遗忘曲线。
#[derive(Clone, Debug, PartialEq)]
pub struct ForgettingCurve {
    /// base 对每个历史 oracle 的保持率。
    pub base: Vec<f64>,
    /// 每步读数。
    pub steps: Vec<ForgettingStep>,
    /// 最后一步后的保持率（没有步骤时为 base）。
    pub final_retention: Vec<f64>,
}

impl ForgettingCurve {
    /// 历史 oracle 数。
    #[must_use]
    pub fn oracles(&self) -> usize {
        self.base.len()
    }

    /// → JSON（`retention` 是「步骤 × oracle」矩阵）。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.forgetting.v1",
            "oracles": self.base.len(),
            "base": self.base,
            "steps": self.steps.iter().map(ForgettingStep::to_json).collect::<Vec<_>>(),
            "final_retention": self.final_retention,
        })
    }
}

fn retention(oracle: &Oracle, net: &Net) -> Result<f64> {
    let len = oracle.len();
    if len == 0 {
        return Ok(1.0);
    }
    let loss = oracle.loss(net)?;
    Ok(1.0 - loss as f64 / len as f64)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        1.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// 逐步应用补丁，记录每一步之后对全部历史 oracle 的保持率。
pub fn forgetting_curve(
    base: &Net,
    steps: &[ForgettingInput],
    history: &[Oracle],
) -> Result<ForgettingCurve> {
    let base_retention = history
        .iter()
        .map(|oracle| retention(oracle, base))
        .collect::<Result<Vec<_>>>()?;
    let mut current = base.clone();
    let mut rows = Vec::with_capacity(steps.len());
    for (index, input) in steps.iter().enumerate() {
        input.patch.apply(&mut current)?;
        let row = history
            .iter()
            .map(|oracle| retention(oracle, &current))
            .collect::<Result<Vec<_>>>()?;
        let loss = input
            .own
            .as_ref()
            .map(|oracle| oracle.loss(&current))
            .transpose()?;
        rows.push(ForgettingStep {
            index,
            loss,
            mean_retention: mean(&row),
            retention: row,
        });
    }
    let final_retention = rows
        .last()
        .map_or_else(|| base_retention.clone(), |row| row.retention.clone());
    Ok(ForgettingCurve {
        base: base_retention,
        steps: rows,
        final_retention,
    })
}
