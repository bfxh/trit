//! 引导遵循度：把简报里的准入、证据规格与义务，和实际观察 / 轨迹对齐。
//!
//! 只判定“是否偏离契约”，不改内核状态；偏离会产出一条 `Escalate` 义务，交给宿主处理。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    AdmissionObligation, Effect, EvidenceVerdict, ObligationKind, ObligationOutcome, Observation,
    Phase, TaskBrief, Ternary, TraceLog, TracePayload,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AdherenceViolation {
    /// 准入未通过（或仍未知）时，已经产生了证据或回执。
    ActedWhileBlocked { admission: Ternary },
    /// 声称完成，但判定仍为 `Neither` 或断言数低于 `produces.min_assertions`。
    EvidenceBelowSpec { required: u32, observed: u32 },
    /// 有行动，但简报列出的义务没有对应的 `ObligationFulfilled` 事件。
    ObligationsIgnored { outstanding: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdherenceReport {
    pub task: String,
    pub violations: Vec<AdherenceViolation>,
    pub escalations: Vec<AdmissionObligation>,
}

impl AdherenceReport {
    pub fn is_compliant(&self) -> bool {
        self.violations.is_empty()
    }

    fn has_acted_while_blocked(&self) -> bool {
        self.violations
            .iter()
            .any(|violation| matches!(violation, AdherenceViolation::ActedWhileBlocked { .. }))
    }
}

/// 只看当前观察：不区分“简报发出前就已存在的在途状态”。
pub fn check_adherence(brief: &TaskBrief, observation: &Observation) -> AdherenceReport {
    evaluate(brief, observation, None)
}

/// 叠加轨迹：只统计 `seq >= since_seq` 的事件，用回执 / 效果事件判断是否真的行动。
pub fn check_adherence_with_trace(
    brief: &TaskBrief,
    observation: &Observation,
    log: &TraceLog,
    since_seq: u64,
) -> AdherenceReport {
    evaluate(brief, observation, Some((log, since_seq)))
}

fn evaluate(
    brief: &TaskBrief,
    observation: &Observation,
    trace: Option<(&TraceLog, u64)>,
) -> AdherenceReport {
    let mut report = AdherenceReport {
        task: brief.task.clone(),
        violations: Vec::new(),
        escalations: Vec::new(),
    };
    let completed = observation.phase == Phase::Settled;
    let has_evidence = observation.evidence.is_some() || !observation.evidence_rails.is_empty();
    if brief.admission != Ternary::True && (completed || has_evidence) {
        push(
            &mut report,
            brief,
            AdherenceViolation::ActedWhileBlocked {
                admission: brief.admission,
            },
        );
    }
    if completed {
        let verdict = observation.judgment(brief.epoch);
        let observed = evidence_checks(observation);
        if verdict == EvidenceVerdict::Neither || observed < brief.produces.min_assertions {
            push(
                &mut report,
                brief,
                AdherenceViolation::EvidenceBelowSpec {
                    required: brief.produces.min_assertions,
                    observed,
                },
            );
        }
    }
    if let Some((log, since_seq)) = trace {
        let acted = acted_after(log, &brief.task, since_seq);
        if acted && brief.admission != Ternary::True && !report.has_acted_while_blocked() {
            push(
                &mut report,
                brief,
                AdherenceViolation::ActedWhileBlocked {
                    admission: brief.admission,
                },
            );
        }
        if acted && !brief.obligations.is_empty() {
            let outstanding = outstanding_obligations(log, brief, since_seq);
            if !outstanding.is_empty() {
                push(
                    &mut report,
                    brief,
                    AdherenceViolation::ObligationsIgnored { outstanding },
                );
            }
        }
    }
    report
}

fn push(report: &mut AdherenceReport, brief: &TaskBrief, violation: AdherenceViolation) {
    let (subject, detail) = match &violation {
        AdherenceViolation::ActedWhileBlocked { admission } => (
            "adherence:acted_while_blocked",
            format!("admission={admission:?} 时产生了行动或证据"),
        ),
        AdherenceViolation::EvidenceBelowSpec { required, observed } => (
            "adherence:evidence_below_spec",
            format!("要求至少 {required} 条断言，实际 {observed}"),
        ),
        AdherenceViolation::ObligationsIgnored { outstanding } => (
            "adherence:obligations_ignored",
            format!("未履行的义务: {outstanding:?}"),
        ),
    };
    report.violations.push(violation);
    report.escalations.push(AdmissionObligation::new(
        ObligationKind::Escalate,
        &brief.task,
        subject,
        &detail,
    ));
}

fn evidence_checks(observation: &Observation) -> u32 {
    let legacy = observation
        .evidence
        .as_ref()
        .map_or(0, |report| report.checks);
    let rails = observation
        .evidence_rails
        .support
        .iter()
        .chain(&observation.evidence_rails.counter)
        .chain(&observation.evidence_rails.inconclusive)
        .map(|item| item.report.checks)
        .max()
        .unwrap_or(0);
    legacy.max(rails)
}

fn acted_after(log: &TraceLog, task: &str, since_seq: u64) -> bool {
    log.events().any(|event| {
        event.seq >= since_seq
            && match event.payload.as_ref() {
                Some(TracePayload::Effect { attempt, state }) => {
                    attempt.task == task
                        && matches!(
                            *state,
                            Effect::Sending | Effect::Sent | Effect::Unknown | Effect::Confirmed
                        )
                }
                Some(TracePayload::Receipt { .. }) => event.correlation_id == task,
                _ => false,
            }
    })
}

fn outstanding_obligations(log: &TraceLog, brief: &TaskBrief, since_seq: u64) -> Vec<String> {
    let mut fulfilled: BTreeSet<&str> = BTreeSet::new();
    for event in log.events().filter(|event| event.seq >= since_seq) {
        if let Some(TracePayload::Obligation {
            outcome: ObligationOutcome::Fulfilled,
            obligation,
        }) = event.payload.as_ref()
        {
            fulfilled.insert(obligation.id.as_str());
        }
    }
    brief
        .obligations
        .iter()
        .filter(|obligation| !fulfilled.contains(obligation.id.as_str()))
        .map(|obligation| obligation.id.clone())
        .collect()
}

/// 跨任务引导遵循度聚合；可在多次 tick 之间累计。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidanceMetrics {
    pub briefs: u64,
    pub tasks_with_violations: u64,
    pub acted_while_blocked: u64,
    pub evidence_below_spec: u64,
    pub obligations_ignored: u64,
}

impl GuidanceMetrics {
    pub fn observe(&mut self, reports: &[AdherenceReport]) {
        for report in reports {
            self.briefs += 1;
            if !report.violations.is_empty() {
                self.tasks_with_violations += 1;
            }
            for violation in &report.violations {
                match violation {
                    AdherenceViolation::ActedWhileBlocked { .. } => self.acted_while_blocked += 1,
                    AdherenceViolation::EvidenceBelowSpec { .. } => self.evidence_below_spec += 1,
                    AdherenceViolation::ObligationsIgnored { .. } => self.obligations_ignored += 1,
                }
            }
        }
    }

    /// 有违规的简报占比（基点）；没有简报时为 0。
    pub fn violation_rate_bp(&self) -> u32 {
        if self.briefs == 0 {
            return 0;
        }
        ((u128::from(self.tasks_with_violations) * 10_000) / u128::from(self.briefs)) as u32
    }
}
