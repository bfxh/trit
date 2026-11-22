use serde::{Deserialize, Serialize};

use super::{CheckResult, EvidenceVerdict};
use crate::Ternary;

/// 验收报告。`epoch` 是验收针对的任务定义版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckReport {
    pub epoch: u64,
    pub checks: u32,
    pub result: CheckResult,
}

impl CheckReport {
    /// 把报告折算成四值判定；冲突必须保持为 `Conflicted`，不能压成 `Neither`。
    pub fn judgment(&self, expected_epoch: u64) -> EvidenceVerdict {
        if self.checks == 0 || self.epoch != expected_epoch {
            return EvidenceVerdict::Neither;
        }
        match self.result {
            CheckResult::Passed => EvidenceVerdict::Proved,
            CheckResult::Refuted => EvidenceVerdict::Refuted,
            CheckResult::Inconclusive => EvidenceVerdict::Neither,
            CheckResult::Conflicted => EvidenceVerdict::Conflicted,
        }
    }

    /// 只有「报告针对当前 epoch、至少跑过一条断言、结果确定」才算数。
    /// 冲突按 `Unknown` 返回，保留给需要三值口径的旧调用。
    pub fn verdict(&self, expected_epoch: u64) -> Ternary {
        self.judgment(expected_epoch).value()
    }
}
