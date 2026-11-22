//! 阻塞原因到义务的路由，以及 `Selection` 上的义务入口。

use std::collections::BTreeSet;

use crate::{BlockReason, Effect, Selection, Ternary};

use super::kind::{AdmissionObligation, ObligationKind, ObligationPolicy};
use super::schedule::{ObligationBatch, ObligationBudget, schedule_obligations};
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
