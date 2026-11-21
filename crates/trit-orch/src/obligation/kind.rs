//! 义务的类型与默认表：七种「把 Unknown 变可判定」的动作，以及它们的成本、优先级、归属。

use serde::{Deserialize, Serialize};
/// 义务的资源估算。默认值来自 [`ObligationKind::default_cost`]，宿主可覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObligationCost {
    pub tokens: u64,
    pub wall: u64,
    /// 0..=100；越高越需要谨慎或人工确认。
    pub risk: u32,
    /// 是否占用人的注意力预算。
    pub human: bool,
}

/// 把 `Unknown` 变成可判定结论的最小动作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationKind {
    /// 缺测量或证据覆盖不足。
    Measure,
    /// 副作用状态不明，先对账。
    Reconcile,
    /// 证据互相冲突，需要裁决。
    Adjudicate,
    /// 输入过期或 epoch 变化，换基重新取证。
    Rebasis,
    /// 上下文或权属转移。
    Handoff,
    /// 等待外部事件或暂时不足的容量。
    Defer,
    /// 执行侧无法决定，需要人授权或做价值判断。
    Escalate,
}

impl ObligationKind {
    /// 稳定短名，用于义务 ID 与日志。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Measure => "measure",
            Self::Reconcile => "reconcile",
            Self::Adjudicate => "adjudicate",
            Self::Rebasis => "rebasis",
            Self::Handoff => "handoff",
            Self::Defer => "defer",
            Self::Escalate => "escalate",
        }
    }

    /// 默认成本估算。不是精确预算，宿主可以用 [`AdmissionObligation::with_cost`] 覆盖。
    pub const fn default_cost(self) -> ObligationCost {
        match self {
            Self::Measure => ObligationCost {
                tokens: 500,
                wall: 1,
                risk: 0,
                human: false,
            },
            Self::Reconcile => ObligationCost {
                tokens: 200,
                wall: 1,
                risk: 20,
                human: false,
            },
            Self::Adjudicate => ObligationCost {
                tokens: 800,
                wall: 2,
                risk: 30,
                human: false,
            },
            Self::Rebasis => ObligationCost {
                tokens: 1_000,
                wall: 2,
                risk: 10,
                human: false,
            },
            Self::Handoff => ObligationCost {
                tokens: 300,
                wall: 1,
                risk: 10,
                human: false,
            },
            Self::Defer => ObligationCost {
                tokens: 0,
                wall: 0,
                risk: 0,
                human: false,
            },
            Self::Escalate => ObligationCost {
                tokens: 0,
                wall: 0,
                risk: 0,
                human: true,
            },
        }
    }

    /// 同截止期下的默认优先级；越大越先。
    pub const fn default_priority(self) -> u32 {
        match self {
            Self::Escalate => 100,
            Self::Reconcile => 90,
            Self::Adjudicate => 80,
            Self::Measure => 60,
            Self::Rebasis => 50,
            Self::Handoff => 40,
            Self::Defer => 10,
        }
    }

    /// 默认归属；`None` 表示交给宿主的默认执行者。
    pub const fn default_owner(self) -> Option<&'static str> {
        match self {
            Self::Reconcile => Some("effect-owner"),
            Self::Adjudicate => Some("adjudicator"),
            Self::Escalate => Some("human"),
            _ => None,
        }
    }
}

/// 一条准入补证义务。`task` 是被阻塞的任务，`subject` 是阻塞来源。
///
/// 与 `context::Obligation` 区分：后者是压缩闸门的具体补证动作，
/// 本类型描述的是准入层“为什么是 Unknown、下一步补什么”。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionObligation {
    /// 稳定 ID：`kind:task:subject`。旧数据为空时，排期前会自动补齐。
    #[serde(default)]
    pub id: String,
    pub kind: ObligationKind,
    pub task: String,
    pub subject: String,
    pub detail: String,
    #[serde(default)]
    pub cost: ObligationCost,
    #[serde(default)]
    pub owner: Option<String>,
    /// 绝对逻辑时间；`None` 表示尚未分配 SLA。
    #[serde(default)]
    pub deadline: Option<u64>,
    #[serde(default)]
    pub priority: u32,
}

impl AdmissionObligation {
    pub fn new(kind: ObligationKind, task: &str, subject: &str, detail: &str) -> Self {
        let task = task.to_owned();
        let subject = subject.to_owned();
        Self {
            id: canonical_id(kind, &task, &subject),
            kind,
            task,
            subject,
            detail: detail.to_owned(),
            cost: kind.default_cost(),
            owner: kind.default_owner().map(str::to_owned),
            deadline: None,
            priority: kind.default_priority(),
        }
    }

    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn with_deadline(mut self, deadline: u64) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub fn with_cost(mut self, cost: ObligationCost) -> Self {
        self.cost = cost;
        self
    }

    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    pub(super) fn ensure_id(&mut self) {
        if self.id.trim().is_empty() {
            self.id = canonical_id(self.kind, &self.task, &self.subject);
        }
    }
}

pub(crate) fn canonical_id(kind: ObligationKind, task: &str, subject: &str) -> String {
    format!("{}:{task}:{subject}", kind.as_str())
}

/// 义务的归属与 SLA 策略。只补齐缺失字段，不覆盖显式值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationPolicy {
    #[serde(default = "default_policy_ttl")]
    pub ttl: u64,
    #[serde(default)]
    pub default_owner: Option<String>,
}

fn default_policy_ttl() -> u64 {
    300
}

impl Default for ObligationPolicy {
    fn default() -> Self {
        Self {
            ttl: default_policy_ttl(),
            default_owner: None,
        }
    }
}

impl ObligationPolicy {
    pub fn assign(
        &self,
        obligations: impl IntoIterator<Item = AdmissionObligation>,
        now: u64,
    ) -> Vec<AdmissionObligation> {
        obligations
            .into_iter()
            .map(|mut obligation| {
                obligation.ensure_id();
                if obligation.owner.is_none() {
                    obligation.owner = self.default_owner.clone();
                }
                if obligation.deadline.is_none() {
                    obligation.deadline = Some(now.saturating_add(self.ttl));
                }
                obligation
            })
            .collect()
    }
}
