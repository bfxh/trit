//! 准入受阻时的最小补证动作，以及它们的成本、归属、截止期与排期。
//!
//! 只有会让准入变成 `Unknown` 的原因才产生义务；`False` 原因
//! （容量上限、明确拒绝）是改计划，不是补证。

use std::cmp::Ordering;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::flip_rates::{FlipRates, ScorePolicy, order_obligations_by_score};
use crate::{BlockReason, Effect, Selection, Ternary};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationKind {
    /// 缺测量或证据覆盖不足。
    Measure,
    /// 副作用状态不明，先对账。
    Reconcile,
    /// 路径未认领 / 已过期 / 被他人占用，先拿 claim。
    Claim,
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
            Self::Claim => "claim",
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
            Self::Claim => ObligationCost {
                tokens: 0,
                wall: 1,
                risk: 0,
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

    /// 默认最大尝试次数；超过后应升级而不是继续重试。
    pub const fn default_max_attempts(self) -> u32 {
        match self {
            Self::Reconcile => 2,
            Self::Escalate | Self::Handoff | Self::Defer => 1,
            _ => 3,
        }
    }

    /// 同截止期下的默认优先级；越大越先。
    pub const fn default_priority(self) -> u32 {
        match self {
            Self::Escalate => 100,
            Self::Claim => 95,
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
            Self::Claim => None,
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
    /// 同一义务允许尝试的次数；由 `ObligationGuard` 计数。
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
}

fn default_max_attempts() -> u32 {
    3
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
            max_attempts: kind.default_max_attempts(),
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

    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    pub(crate) fn ensure_id(&mut self) {
        if self.id.trim().is_empty() {
            self.id = canonical_id(self.kind, &self.task, &self.subject);
        }
    }
}

fn canonical_id(kind: ObligationKind, task: &str, subject: &str) -> String {
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

/// 一次排期可用的机器 token 与人的注意力名额。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObligationBudget {
    pub tokens: u64,
    pub human: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ObligationSkipReason {
    Duplicate,
    OverBudget {
        required: u64,
        available: u64,
    },
    NoHuman,
    /// 同一义务重复尝试超过上限，应升级为 Handoff/Escalate。
    Thrashed {
        attempts: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationSkip {
    pub id: String,
    pub kind: ObligationKind,
    pub reason: ObligationSkipReason,
}

/// 一轮义务排期的结果。`overdue` 里的义务仍会先排，但宿主应知道它们已经过期。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationBatch {
    pub scheduled: Vec<AdmissionObligation>,
    pub skipped: Vec<ObligationSkip>,
    pub overdue: Vec<String>,
    pub tokens_used: u64,
    pub human_used: usize,
}

/// 按截止期、优先级、成本排期；预算不足时跳过该项，继续尝试后面的小项。
///
/// 排序：有截止期的先于无截止期的；截止期近的先；同截止期按优先级降序，
/// 再按 token 成本升序，最后按 ID 升序。`now` 只用于标记 overdue，不修改顺序。
pub fn schedule_obligations(
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    now: u64,
    budget: &ObligationBudget,
) -> ObligationBatch {
    schedule_obligations_with_order(obligations, now, budget, order_obligations)
}

/// 按 `P(flip) × 优先级 / 成本` 排序；截止期只做同分 tie-break。
pub fn schedule_obligations_by_score(
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    now: u64,
    budget: &ObligationBudget,
    rates: &FlipRates,
    policy: &ScorePolicy,
) -> ObligationBatch {
    schedule_obligations_with_order(obligations, now, budget, |left, right| {
        order_obligations_by_score(left, right, rates, policy)
    })
}

fn schedule_obligations_with_order(
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    now: u64,
    budget: &ObligationBudget,
    order: impl Fn(&AdmissionObligation, &AdmissionObligation) -> Ordering,
) -> ObligationBatch {
    let mut obligations: Vec<AdmissionObligation> = obligations.into_iter().collect();
    for obligation in &mut obligations {
        obligation.ensure_id();
    }
    obligations.sort_by(order);

    let mut batch = ObligationBatch::default();
    let mut seen = BTreeSet::new();
    let mut tokens_left = budget.tokens;
    let mut human_left = budget.human;

    for obligation in obligations {
        if !seen.insert(obligation.id.clone()) {
            batch
                .skipped
                .push(skip(&obligation, ObligationSkipReason::Duplicate));
            continue;
        }
        if obligation.cost.human && human_left == 0 {
            batch
                .skipped
                .push(skip(&obligation, ObligationSkipReason::NoHuman));
            continue;
        }
        if obligation.cost.tokens > tokens_left {
            batch.skipped.push(skip(
                &obligation,
                ObligationSkipReason::OverBudget {
                    required: obligation.cost.tokens,
                    available: tokens_left,
                },
            ));
            continue;
        }

        if obligation.deadline.is_some_and(|deadline| deadline <= now) {
            batch.overdue.push(obligation.id.clone());
        }
        tokens_left -= obligation.cost.tokens;
        if obligation.cost.human {
            human_left -= 1;
            batch.human_used += 1;
        }
        batch.tokens_used = batch.tokens_used.saturating_add(obligation.cost.tokens);
        batch.scheduled.push(obligation);
    }
    batch
}

fn skip(obligation: &AdmissionObligation, reason: ObligationSkipReason) -> ObligationSkip {
    ObligationSkip {
        id: obligation.id.clone(),
        kind: obligation.kind,
        reason,
    }
}

pub(crate) fn order_obligations(
    left: &AdmissionObligation,
    right: &AdmissionObligation,
) -> Ordering {
    match (left.deadline, right.deadline) {
        (Some(left_deadline), Some(right_deadline)) => left_deadline
            .cmp(&right_deadline)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| left.cost.tokens.cmp(&right.cost.tokens))
            .then_with(|| left.id.cmp(&right.id)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.cost.tokens.cmp(&right.cost.tokens))
            .then_with(|| left.id.cmp(&right.id)),
    }
}

impl Selection {
    /// 收集所有决定上的义务，按 ID 去重，保持决定顺序。
    pub fn obligations(&self) -> Vec<AdmissionObligation> {
        let mut seen = BTreeSet::new();
        let mut obligations = Vec::new();
        for decision in &self.decisions {
            for obligation in &decision.obligations {
                let mut obligation = obligation.clone();
                obligation.ensure_id();
                if seen.insert(obligation.id.clone()) {
                    obligations.push(obligation);
                }
            }
        }
        obligations
    }

    /// 先按策略补齐 owner/deadline，再做预算内排期。
    pub fn schedule_obligations(
        &self,
        policy: &ObligationPolicy,
        now: u64,
        budget: &ObligationBudget,
    ) -> ObligationBatch {
        schedule_obligations(policy.assign(self.obligations(), now), now, budget)
    }
}

/// 把一组阻塞原因折算成义务，按 `(kind, subject)` 去重。
///
/// 调用方只应在 `admission == Unknown` 时调用。无法分类的未知原因会兜底成
/// 一条 `Defer`，保证“不存在无义务的 U”是结构性约束，而不是当前原因表的巧合。
pub fn obligations_for(task: &str, reasons: &[BlockReason]) -> Vec<AdmissionObligation> {
    let mut obligations = Vec::new();
    for reason in reasons {
        let obligation = match reason {
            BlockReason::Dependency {
                task: dependency,
                required,
                verdict,
            } => {
                if *required == Ternary::Unknown {
                    AdmissionObligation::new(
                        ObligationKind::Defer,
                        task,
                        dependency,
                        "依赖是否需要尚未判定",
                    )
                } else if *verdict == Ternary::Unknown {
                    AdmissionObligation::new(
                        ObligationKind::Measure,
                        task,
                        dependency,
                        "依赖尚未产生可判定证据",
                    )
                } else {
                    continue;
                }
            }
            BlockReason::Authorization {
                verdict: Ternary::Unknown,
            } => AdmissionObligation::new(
                ObligationKind::Escalate,
                task,
                "authorization",
                "授权无法由执行侧判定",
            ),
            BlockReason::Claim { verdict } => AdmissionObligation::new(
                ObligationKind::Claim,
                task,
                "claim",
                match *verdict {
                    Ternary::False => "路径被其他 owner 认领，等待或协商",
                    _ => "路径未认领或认领已过期，先 claim",
                },
            ),
            BlockReason::Conflict { task: dependency } => AdmissionObligation::new(
                ObligationKind::Adjudicate,
                task,
                dependency,
                "依赖证据冲突，需要裁决",
            ),
            BlockReason::Freshness { .. } => AdmissionObligation::new(
                ObligationKind::Rebasis,
                task,
                "freshness",
                "输入已过期或新鲜度未知",
            ),
            BlockReason::Effect {
                state: Effect::Sending | Effect::Sent | Effect::Unknown,
            } => AdmissionObligation::new(
                ObligationKind::Reconcile,
                task,
                task,
                "副作用可能已发生，先对账再决定是否重发",
            ),
            BlockReason::Resources { .. }
            | BlockReason::Workers { .. }
            | BlockReason::Tokens { .. } => {
                AdmissionObligation::new(ObligationKind::Defer, task, "resources", "容量暂时不可用")
            }
            _ => continue,
        };
        if !obligations.iter().any(|existing: &AdmissionObligation| {
            existing.kind == obligation.kind && existing.subject == obligation.subject
        }) {
            obligations.push(obligation);
        }
    }
    if obligations.is_empty() {
        obligations.push(AdmissionObligation::new(
            ObligationKind::Defer,
            task,
            "*",
            "未能分类的未知准入原因",
        ));
    }
    obligations
}
