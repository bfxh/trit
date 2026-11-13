use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::add_tokens;
use crate::{AdmissionObligation, Effect, Error, Resources, Task, Ternary};

/// 一个任务的完整定义。`epoch` 是该定义自己的版本号：补丁替换定义时递增，
/// 旧 epoch 的执行与验收结果因此自动失效，与全局计划 revision 解耦。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub task: Task,
    pub requirements: Resources,
    pub max_tokens: u64,
    #[serde(default)]
    pub epoch: u64,
}

/// 一次有限资源的准入上限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub workers: usize,
    pub capacity: Resources,
    pub tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Pending,
    InFlight,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckResult {
    Passed,
    Refuted,
    Inconclusive,
    /// 多个来源对同一命题给出相反结论；这是冲突，不是“证据不足”。
    Conflicted,
}

/// 单条证据折算出的四值判定（Belnap 形状）。
///
/// `Neither` 对应“没有可用证据”，`Conflicted` 对应“支持和反对都有”。
/// 两者都不能解锁下游，但出口不同：前者补证，后者裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceVerdict {
    Neither,
    Proved,
    Refuted,
    Conflicted,
}

impl EvidenceVerdict {
    pub const fn value(self) -> Ternary {
        match self {
            Self::Proved => Ternary::True,
            Self::Refuted => Ternary::False,
            Self::Neither | Self::Conflicted => Ternary::Unknown,
        }
    }

    pub const fn is_conflicted(self) -> bool {
        matches!(self, Self::Conflicted)
    }
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub fresh: u64,
    pub cached: u64,
    pub output: u64,
}

impl TokenUsage {
    pub fn total(&self) -> Result<u64, Error> {
        self.total_for(None)
    }

    pub(crate) fn total_for(&self, task: Option<&str>) -> Result<u64, Error> {
        add_tokens(
            add_tokens(self.fresh, self.cached, task)?,
            self.output,
            task,
        )
    }

    pub(crate) fn checked_add(self, other: Self, task: &str) -> Result<Self, Error> {
        Ok(Self {
            fresh: add_tokens(self.fresh, other.fresh, Some(task))?,
            cached: add_tokens(self.cached, other.cached, Some(task))?,
            output: add_tokens(self.output, other.output, Some(task))?,
        })
    }
}

/// 某个任务在快照中的状态。`epoch` 是这次执行所依据的任务定义版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub phase: Phase,
    #[serde(default)]
    pub epoch: u64,
    pub authorization: Ternary,
    pub freshness: Ternary,
    pub evidence: Option<CheckReport>,
    pub usage: TokenUsage,
}

impl Default for Observation {
    fn default() -> Self {
        Self {
            phase: Phase::Pending,
            epoch: 0,
            authorization: Ternary::Unknown,
            freshness: Ternary::Unknown,
            evidence: None,
            usage: TokenUsage::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: u64,
    pub observations: BTreeMap<String, Observation>,
}

/// 一次计划补丁：以某个 revision 为基线，增删改任务定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPatch {
    pub base_revision: u64,
    #[serde(default)]
    pub add: Vec<TaskSpec>,
    #[serde(default)]
    pub remove: Vec<String>,
    #[serde(default)]
    pub replace: Vec<TaskSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockReason {
    Dependency {
        task: String,
        required: Ternary,
        verdict: Ternary,
    },
    Authorization {
        verdict: Ternary,
    },
    Freshness {
        verdict: Ternary,
    },
    /// 效果台账里这个任务的操作可能已发生，或已经发生。
    Effect {
        state: Effect,
    },
    /// 依赖的证据同时包含支持与反对，需要裁决。
    Conflict {
        task: String,
    },
    ResourceCapacity {
        requirements: Resources,
        capacity: Resources,
    },
    Resources {
        requirements: Resources,
        available: Resources,
    },
    Workers {
        occupied: usize,
        limit: usize,
    },
    TokenCapacity {
        required: u64,
        limit: u64,
    },
    Tokens {
        required: u64,
        available: u64,
    },
    InFlight,
    Settled,
    /// 已完成，但结果属于旧 epoch；定义已换，可重新派发。
    Superseded {
        run_epoch: u64,
        task_epoch: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub task: String,
    pub admission: Ternary,
    pub reasons: Vec<BlockReason>,
    /// `Unknown` 准入的最小补证动作；`False`/`True` 与状态类决定为空。
    #[serde(default)]
    pub obligations: Vec<AdmissionObligation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub revision: u64,
    pub selected: Vec<String>,
    pub decisions: Vec<Decision>,
    pub observed_tokens: TokenUsage,
    pub reserved_tokens: u64,
    pub reserved_resources: Resources,
    pub occupied_workers: usize,
}
