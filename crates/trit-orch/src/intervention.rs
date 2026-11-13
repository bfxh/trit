//! 跨任务干预：把单任务的遵循度偏离收敛成分级、去重的干预计划。
//!
//! 安全类偏离优先：被隔离的任务不再叠加补证义务；证据缺口转成可任务化的 Measure，
//! 其余情况升级为一次分组 Escalate。策略只产出计划，不直接改选批或台账。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{AdherenceReport, AdherenceViolation, AdmissionObligation, ObligationKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionAction {
    /// 隔离任务，先对账 / 人工确认，再允许重新进入派发。
    Quarantine,
    /// 交给人的分组升级。
    Escalate,
    /// 证据缺口，转成可任务化的 Measure。
    RequireEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InterventionPolicy {
    /// 安全类偏离是否隔离相关任务；关闭后只升级，不进入 `held`。
    pub quarantine_unsafe: bool,
}

impl Default for InterventionPolicy {
    fn default() -> Self {
        Self {
            quarantine_unsafe: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intervention {
    pub action: InterventionAction,
    pub tasks: Vec<String>,
    pub subject: String,
    pub detail: String,
    pub obligation: AdmissionObligation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterventionPlan {
    pub interventions: Vec<Intervention>,
    pub obligations: Vec<AdmissionObligation>,
    /// 宿主在干预解除前不应重新派发的任务。
    pub held: Vec<String>,
}

impl InterventionPlan {
    pub fn is_empty(&self) -> bool {
        self.interventions.is_empty()
    }
}

pub fn plan_interventions(
    reports: &[AdherenceReport],
    policy: &InterventionPolicy,
) -> InterventionPlan {
    let mut acted: BTreeSet<String> = BTreeSet::new();
    let mut ignored: BTreeSet<String> = BTreeSet::new();
    let mut below: BTreeSet<String> = BTreeSet::new();
    for report in reports {
        for violation in &report.violations {
            match violation {
                AdherenceViolation::ActedWhileBlocked { .. } => {
                    acted.insert(report.task.clone());
                }
                AdherenceViolation::ObligationsIgnored { .. } => {
                    ignored.insert(report.task.clone());
                }
                AdherenceViolation::EvidenceBelowSpec { .. } => {
                    below.insert(report.task.clone());
                }
            }
        }
    }
    for task in &acted {
        ignored.remove(task);
        below.remove(task);
    }
    for task in &ignored {
        below.remove(task);
    }

    let mut plan = InterventionPlan::default();
    if !acted.is_empty() {
        let tasks: Vec<String> = acted.into_iter().collect();
        let held = policy.quarantine_unsafe;
        let subject = if held {
            "intervention:quarantine"
        } else {
            "intervention:acted_while_blocked"
        };
        let detail = format!("准入未通过时仍产生行动 / 证据；任务: {tasks:?}");
        let obligation =
            AdmissionObligation::new(ObligationKind::Escalate, &tasks[0], subject, &detail);
        if held {
            plan.held.extend(tasks.clone());
        }
        plan.interventions.push(Intervention {
            action: if held {
                InterventionAction::Quarantine
            } else {
                InterventionAction::Escalate
            },
            tasks,
            subject: subject.into(),
            detail,
            obligation: obligation.clone(),
        });
        plan.obligations.push(obligation);
    }

    if !ignored.is_empty() {
        let tasks: Vec<String> = ignored.into_iter().collect();
        let detail = format!("有行动但义务未履行；任务: {tasks:?}");
        let obligation = AdmissionObligation::new(
            ObligationKind::Escalate,
            &tasks[0],
            "intervention:obligations_ignored",
            &detail,
        );
        plan.interventions.push(Intervention {
            action: InterventionAction::Escalate,
            tasks,
            subject: "intervention:obligations_ignored".into(),
            detail,
            obligation: obligation.clone(),
        });
        plan.obligations.push(obligation);
    }

    for task in below {
        let detail = format!("证据低于 produces.min_assertions；任务: {task}");
        let obligation = AdmissionObligation::new(
            ObligationKind::Measure,
            &task,
            "intervention:evidence_gap",
            &detail,
        );
        plan.interventions.push(Intervention {
            action: InterventionAction::RequireEvidence,
            tasks: vec![task],
            subject: "intervention:evidence_gap".into(),
            detail,
            obligation: obligation.clone(),
        });
        plan.obligations.push(obligation);
    }

    plan
}
