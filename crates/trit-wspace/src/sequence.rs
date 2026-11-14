//! 多任务连续编辑协议：逐步提交、每步检查历史保持集、失败原样回滚。
//!
//! 单次编辑的 `certify` 只看一次；连续编辑要回答的是「第 k 步之后，前 k−1 个任务还在不在」。
//! 这里把协议钉死：
//!
//! - 每一步带自己的 `keep`（该步之后必须保持的历史 oracle 集合）与可选 `edit`（本步目标）；
//! - 逐步应用后**先检查再提交**：`keep` 总失配 ≤ `allow` 且 `edit` 全部达成才落地；
//! - 不达标就**原样回滚**到上一步（不是整段回滚），并记录该步的读数；
//! - 最终工件与逐步读数一起返回，可直接接 `audit` / `forgetting` 复核。

use serde_json::{Value, json};

use crate::edit::Patch;
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;

/// 一步输入。
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceStep {
    /// 步骤名（写进报告与审计）。
    pub name: String,
    /// 本步补丁。
    pub patch: Patch,
    /// 本步之后必须保持的 oracle 集合（通常含历史任务）。
    pub keep: Vec<Oracle>,
    /// 本步要达成的编辑集（可选）。
    pub edit: Option<Oracle>,
}

/// 一步的读数。
#[derive(Clone, Debug, PartialEq)]
pub struct StepReport {
    /// 步骤号（从 0 开始）。
    pub index: usize,
    /// 步骤名。
    pub name: String,
    /// 是否落地。
    pub applied: bool,
    /// 是否因不达标而回滚。
    pub rolled_back: bool,
    /// `keep` 集合上的总失配数。
    pub keep_loss: usize,
    /// 本步编辑集上的失配数（没给就是 `None`）。
    pub edit_loss: Option<usize>,
}

impl StepReport {
    fn to_json(&self) -> Value {
        json!({
            "index": self.index,
            "name": self.name,
            "applied": self.applied,
            "rolled_back": self.rolled_back,
            "keep_loss": self.keep_loss,
            "edit_loss": self.edit_loss,
        })
    }
}

/// 整段序列的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceOutcome {
    /// 逐步读数。
    pub steps: Vec<StepReport>,
    /// 最终工件（失败步骤已回滚）。
    pub final_net: Net,
    /// 落地步数。
    pub applied: usize,
    /// 回滚步数。
    pub rolled_back: usize,
}

impl SequenceOutcome {
    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema": "trit-wspace.sequence.v1",
            "steps": self.steps.iter().map(StepReport::to_json).collect::<Vec<_>>(),
            "applied": self.applied,
            "rolled_back": self.rolled_back,
            "final_checksum": format!("{:016x}", crate::codec::checksum(&self.final_net)),
        })
    }
}

/// 执行序列：逐步应用、检查、失败回滚到上一步。
pub fn run_sequence(base: &Net, steps: &[SequenceStep], allow: usize) -> Result<SequenceOutcome> {
    let mut current = base.clone();
    let mut reports = Vec::with_capacity(steps.len());
    let mut applied = 0usize;
    let mut rolled_back = 0usize;
    for (index, step) in steps.iter().enumerate() {
        let mut trial = current.clone();
        step.patch.apply(&mut trial)?;
        let mut keep_loss = 0usize;
        for oracle in &step.keep {
            keep_loss += oracle.loss(&trial)?;
        }
        let edit_loss = step
            .edit
            .as_ref()
            .map(|oracle| oracle.loss(&trial))
            .transpose()?;
        let passed = keep_loss <= allow && edit_loss.is_none_or(|loss| loss == 0);
        if passed {
            current = trial;
            applied += 1;
        } else {
            rolled_back += 1;
        }
        reports.push(StepReport {
            index,
            name: step.name.clone(),
            applied: passed,
            rolled_back: !passed,
            keep_loss,
            edit_loss,
        });
    }
    Ok(SequenceOutcome {
        steps: reports,
        final_net: current,
        applied,
        rolled_back,
    })
}
