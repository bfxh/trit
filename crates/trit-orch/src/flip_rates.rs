//! 用回放语料估计 P(flip)：同类义务把 Unknown 翻成 T/F 的概率。
//!
//! 样本来自轨迹里的 `ObligationFulfilled` + 同一 task 前后两条 `JudgmentDerived`：
//! 前一条必须是 `Neither`，后一条是 `Proved` / `Refuted` 才算 flip。

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    AdmissionObligation, EvidenceVerdict, ObligationKind, ObligationOutcome, TraceLog, TracePayload,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct FlipCounts {
    resolved: u64,
    flipped: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlipRates {
    counts: BTreeMap<ObligationKind, FlipCounts>,
}

impl FlipRates {
    pub fn record(&mut self, kind: ObligationKind, flipped: bool) {
        let counts = self.counts.entry(kind).or_default();
        counts.resolved += 1;
        if flipped {
            counts.flipped += 1;
        }
    }

    pub fn resolved(&self, kind: ObligationKind) -> u64 {
        self.counts.get(&kind).map_or(0, |counts| counts.resolved)
    }

    pub fn flipped(&self, kind: ObligationKind) -> u64 {
        self.counts.get(&kind).map_or(0, |counts| counts.flipped)
    }

    /// 拉普拉斯平滑后的 P(flip)，单位基点；无样本时返回 5000。
    pub fn probability_bp(&self, kind: ObligationKind) -> u32 {
        let counts = self.counts.get(&kind).copied().unwrap_or_default();
        let numerator = (counts.flipped + 1) * 10_000;
        let denominator = counts.resolved + 2;
        (numerator / denominator) as u32
    }

    pub fn score(&self, obligation: &AdmissionObligation, policy: &ScorePolicy) -> u64 {
        let numerator =
            u128::from(self.probability_bp(obligation.kind)) * u128::from(obligation.priority);
        let denominator = u128::from(policy.cost(obligation));
        u64::try_from(numerator / denominator).unwrap_or(u64::MAX)
    }

    /// 从轨迹学习：只看已 Fulfilled、且 task 前 U 后 T/F 的样本。
    pub fn from_trace(log: &TraceLog) -> Self {
        let judgments: Vec<(u64, &str, EvidenceVerdict)> = log
            .events()
            .filter_map(|event| match event.payload.as_ref()? {
                TracePayload::Judgment {
                    subject, verdict, ..
                } => Some((event.seq, subject.as_str(), *verdict)),
                _ => None,
            })
            .collect();

        let mut rates = Self::default();
        for event in log.events() {
            let Some(TracePayload::Obligation {
                outcome: ObligationOutcome::Fulfilled,
                obligation,
            }) = event.payload.as_ref()
            else {
                continue;
            };
            let task = obligation.task.as_str();
            let Some(before) = judgments
                .iter()
                .filter(|(seq, subject, _)| *subject == task && *seq < event.seq)
                .max_by_key(|(seq, _, _)| *seq)
            else {
                continue;
            };
            if before.2 != EvidenceVerdict::Neither {
                continue;
            }
            let Some(after) = judgments
                .iter()
                .filter(|(seq, subject, _)| *subject == task && *seq > event.seq)
                .min_by_key(|(seq, _, _)| *seq)
            else {
                continue;
            };
            rates.record(
                obligation.kind,
                matches!(after.2, EvidenceVerdict::Proved | EvidenceVerdict::Refuted),
            );
        }
        rates
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScorePolicy {
    pub risk_weight: u64,
    pub wall_weight: u64,
    pub human_weight: u64,
}

impl Default for ScorePolicy {
    fn default() -> Self {
        Self {
            risk_weight: 10,
            wall_weight: 10,
            human_weight: 1_000,
        }
    }
}

impl ScorePolicy {
    fn cost(&self, obligation: &AdmissionObligation) -> u64 {
        obligation
            .cost
            .tokens
            .saturating_add(obligation.cost.wall.saturating_mul(self.wall_weight))
            .saturating_add(u64::from(obligation.cost.risk).saturating_mul(self.risk_weight))
            .saturating_add(if obligation.cost.human {
                self.human_weight
            } else {
                0
            })
            .max(1)
    }
}

/// 评分主序：概率 × 优先级 / 成本；同分按截止期、token 成本、ID。
pub(crate) fn order_obligations_by_score(
    left: &AdmissionObligation,
    right: &AdmissionObligation,
    rates: &FlipRates,
    policy: &ScorePolicy,
) -> Ordering {
    rates
        .score(right, policy)
        .cmp(&rates.score(left, policy))
        .then_with(|| deadline_order(left.deadline, right.deadline))
        .then_with(|| left.cost.tokens.cmp(&right.cost.tokens))
        .then_with(|| left.id.cmp(&right.id))
}

fn deadline_order(left: Option<u64>, right: Option<u64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
