//! 内核主循环：选批 → 简报 → 遵循度 → 义务汇总。
//!
//! 派发仍由宿主完成：先用 `prepare_effects` 落意图，再调用 `dispatch_ready`。

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    AdherenceReport, AdmissionObligation, ClaimSet, Error, GuidanceMetrics, InterventionPlan,
    InterventionPolicy, Ledger, Limits, Phase, Plan, Selection, Snapshot, TaskBrief, TraceLog,
    check_adherence, check_adherence_with_trace, plan_interventions, task_brief,
};

pub struct TickInput<'a> {
    pub plan: &'a Plan,
    pub snapshot: &'a Snapshot,
    pub limits: &'a Limits,
    pub ledger: &'a Ledger,
    pub claims: Option<&'a ClaimSet<'a>>,
    /// 上一轮发出的简报；只对 epoch 仍一致的任务做遵循度检查。
    pub briefed: &'a [TaskBrief],
    /// 简报发出后的轨迹；`None` 时只做状态检查。
    pub trace: Option<&'a TraceLog>,
    pub since_seq: u64,
    pub intervention: &'a InterventionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick {
    pub revision: u64,
    pub selection: Selection,
    /// 本轮选中任务的简报，顺序跟随选批结果。
    pub briefs: Vec<TaskBrief>,
    /// 上一轮简报的遵循度报告，按任务 ID 排序。
    pub adherence: Vec<AdherenceReport>,
    /// 跨任务干预计划；`held` 里的任务在解除前不应重新派发。
    pub interventions: InterventionPlan,
    /// 本轮遵循度聚合，可被宿主累计后计算长期比率。
    pub guidance_metrics: GuidanceMetrics,
    /// 选批义务 + 干预义务，按 ID 去重。
    pub obligations: Vec<AdmissionObligation>,
}

pub fn tick(input: TickInput<'_>) -> Result<Tick, Error> {
    let selection = match input.claims {
        Some(claims) => input.plan.select_with_effects_and_claims(
            input.snapshot,
            input.limits,
            input.ledger,
            claims,
        )?,
        None => input
            .plan
            .select_with_effects(input.snapshot, input.limits, input.ledger)?,
    };

    let mut briefs = Vec::with_capacity(selection.selected.len());
    for task in &selection.selected {
        briefs.push(task_brief(input.plan, &selection, task)?);
    }

    let adherence = check_briefed(&input);
    let mut guidance_metrics = GuidanceMetrics::default();
    guidance_metrics.observe(&adherence);
    let interventions = plan_interventions(&adherence, input.intervention);
    let mut seen = BTreeSet::new();
    let mut obligations = Vec::new();
    for obligation in selection
        .obligations()
        .into_iter()
        .chain(interventions.obligations.iter().cloned())
    {
        if seen.insert(obligation.id.clone()) {
            obligations.push(obligation);
        }
    }

    Ok(Tick {
        revision: input.plan.revision(),
        selection,
        briefs,
        adherence,
        interventions,
        guidance_metrics,
        obligations,
    })
}

fn check_briefed(input: &TickInput<'_>) -> Vec<AdherenceReport> {
    let mut latest: BTreeMap<&str, &TaskBrief> = BTreeMap::new();
    for brief in input.briefed {
        match latest.get(brief.task.as_str()) {
            Some(existing) if existing.epoch >= brief.epoch => {}
            _ => {
                latest.insert(&brief.task, brief);
            }
        }
    }

    let mut reports = Vec::new();
    for brief in latest.into_values() {
        let Some(spec) = input.plan.task(&brief.task) else {
            continue;
        };
        if spec.epoch != brief.epoch {
            continue;
        }
        let Some(observation) = input.snapshot.observations.get(&brief.task) else {
            continue;
        };
        if observation.phase == Phase::Settled && observation.epoch != brief.epoch {
            continue;
        }
        let report = match input.trace {
            Some(log) => check_adherence_with_trace(brief, observation, log, input.since_seq),
            None => check_adherence(brief, observation),
        };
        reports.push(report);
    }
    reports
}
