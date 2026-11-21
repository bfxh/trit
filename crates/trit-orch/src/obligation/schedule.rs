//! 义务排期：排序规则与 token / 人的注意力预算。

use std::cmp::Ordering;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::kind::{AdmissionObligation, ObligationKind};
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
    OverBudget { required: u64, available: u64 },
    NoHuman,
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
    let mut obligations: Vec<AdmissionObligation> = obligations.into_iter().collect();
    for obligation in &mut obligations {
        obligation.ensure_id();
    }
    obligations.sort_by(order_obligations);

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

fn order_obligations(left: &AdmissionObligation, right: &AdmissionObligation) -> Ordering {
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
