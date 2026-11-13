//! 义务抖动上限：记录尝试次数，超限后跳过并给出升级路径。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::flip_rates::{FlipRates, ScorePolicy};
use crate::obligation::{
    AdmissionObligation, ObligationBatch, ObligationBudget, ObligationKind, ObligationSkip,
    ObligationSkipReason, schedule_obligations, schedule_obligations_by_score,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationGuard {
    attempts: BTreeMap<String, u32>,
}

impl ObligationGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attempts(&self, id: &str) -> u32 {
        self.attempts.get(id).copied().unwrap_or(0)
    }

    pub fn record_attempt(&mut self, obligation: &AdmissionObligation) {
        let attempts = self.attempts.entry(obligation.id.clone()).or_default();
        *attempts = attempts.saturating_add(1);
    }

    /// 义务已解决：清空计数，后续同 ID 的义务重新开始。
    pub fn record_fulfilled(&mut self, obligation: &AdmissionObligation) {
        self.attempts.remove(&obligation.id);
    }

    pub fn is_thrashed(&self, obligation: &AdmissionObligation) -> bool {
        self.attempts(&obligation.id) >= obligation.max_attempts
    }

    /// 超限后的升级路径；Escalate / Handoff / Defer 本身是终点，返回 None。
    pub fn escalation_for(&self, obligation: &AdmissionObligation) -> Option<AdmissionObligation> {
        let attempts = self.attempts(&obligation.id);
        let (kind, detail) = match obligation.kind {
            ObligationKind::Reconcile => (
                ObligationKind::Handoff,
                format!("对账 {attempts} 次未决，转交接/人工"),
            ),
            ObligationKind::Escalate | ObligationKind::Handoff | ObligationKind::Defer => {
                return None;
            }
            _ => (
                ObligationKind::Escalate,
                format!("义务 {attempts} 次未决，升级人工"),
            ),
        };
        Some(
            AdmissionObligation::new(kind, &obligation.task, &obligation.subject, &detail)
                .with_priority(100)
                .with_max_attempts(1),
        )
    }

    fn split(
        &self,
        obligations: impl IntoIterator<Item = AdmissionObligation>,
    ) -> (Vec<AdmissionObligation>, Vec<ObligationSkip>) {
        let mut available = Vec::new();
        let mut skipped = Vec::new();
        for obligation in obligations {
            if self.is_thrashed(&obligation) {
                skipped.push(ObligationSkip {
                    id: obligation.id.clone(),
                    kind: obligation.kind,
                    reason: ObligationSkipReason::Thrashed {
                        attempts: self.attempts(&obligation.id),
                    },
                });
            } else {
                available.push(obligation);
            }
        }
        (available, skipped)
    }
}

pub fn schedule_obligations_with_guard(
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    now: u64,
    budget: &ObligationBudget,
    guard: &ObligationGuard,
) -> ObligationBatch {
    let (available, skipped) = guard.split(obligations);
    let mut batch = schedule_obligations(available, now, budget);
    batch.skipped.extend(skipped);
    batch
}

pub fn schedule_obligations_by_score_with_guard(
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    now: u64,
    budget: &ObligationBudget,
    rates: &FlipRates,
    policy: &ScorePolicy,
    guard: &ObligationGuard,
) -> ObligationBatch {
    let (available, skipped) = guard.split(obligations);
    let mut batch = schedule_obligations_by_score(available, now, budget, rates, policy);
    batch.skipped.extend(skipped);
    batch
}
