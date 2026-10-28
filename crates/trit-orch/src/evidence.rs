use serde::{Deserialize, Serialize};

use crate::{CheckReport, CheckResult};

/// 一次执行的结束方式。`Signalled`/`TimedOut`/`SpawnFailed` 都不是断言失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exit {
    Completed { code: i32 },
    Signalled,
    TimedOut,
    SpawnFailed,
}

/// 受信 executor 返回的回执；不是模型自述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub epoch: u64,
    pub exit: Exit,
    pub assertions_passed: u32,
    pub assertions_failed: u32,
}

/// 验收规格：至少要通过多少条断言才算覆盖命题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSpec {
    pub min_assertions: u32,
}

impl Receipt {
    /// 把回执折算成验收报告。只有「有失败断言」才算 `Refuted`；
    /// 崩溃、超时、零断言、覆盖不足一律 `Inconclusive`，因为都不能证伪命题。
    ///
    /// 零断言在这里和 `CheckReport::verdict` 各有一道守卫（双保险）：删掉任一道，
    /// 零检查仍无法变成 Support。
    pub fn report(&self, spec: &EvidenceSpec) -> CheckReport {
        let checks = self
            .assertions_passed
            .saturating_add(self.assertions_failed);
        let result = if self.assertions_failed > 0 {
            CheckResult::Refuted
        } else {
            match self.exit {
                Exit::Completed { code: 0 } if self.assertions_passed >= spec.min_assertions => {
                    CheckResult::Passed
                }
                _ => CheckResult::Inconclusive,
            }
        };
        CheckReport {
            epoch: self.epoch,
            checks,
            result,
        }
    }
}
