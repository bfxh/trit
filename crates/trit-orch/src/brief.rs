//! 任务简报：把计划、选批与义务翻译成智能体可执行的引导契约。
//!
//! 简报只读内核状态，不改变准入；宿主负责把它渲染成提示或工具调用参数。

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::{
    AdmissionObligation, BlockReason, EffectSpec, Error, EvidenceSpec, Plan, Resources, Selection,
    TaskGuidance, Ternary,
};

/// 单个任务的引导快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBrief {
    pub task: String,
    pub epoch: u64,
    pub guidance: TaskGuidance,
    pub resolves: Vec<String>,
    pub produces: EvidenceSpec,
    pub effects: Vec<EffectSpec>,
    pub requirements: Resources,
    pub max_tokens: u64,
    pub admission: Ternary,
    pub granted_tokens: u64,
    pub blockers: Vec<BlockReason>,
    /// 当前阻塞对应的补证义务；已可派发时为空。
    pub obligations: Vec<AdmissionObligation>,
}

/// 从计划与选批结果生成任务简报；两者 revision 必须一致。
pub fn task_brief(plan: &Plan, selection: &Selection, task: &str) -> Result<TaskBrief, Error> {
    if selection.revision != plan.revision() {
        return Err(Error::RevisionMismatch {
            expected: plan.revision(),
            actual: selection.revision,
        });
    }
    let spec = plan.task(task).ok_or_else(|| Error::BriefIncomplete {
        task: task.to_owned(),
        reason: "任务不在计划里".into(),
    })?;
    let decision = selection
        .decisions
        .iter()
        .find(|decision| decision.task == task)
        .ok_or_else(|| Error::BriefIncomplete {
            task: task.to_owned(),
            reason: "选批结果缺少该任务的决定".into(),
        })?;
    Ok(TaskBrief {
        task: task.to_owned(),
        epoch: spec.epoch,
        guidance: spec.guidance.clone(),
        resolves: spec.resolves.clone(),
        produces: spec.produces,
        effects: spec.effects.clone(),
        requirements: spec.requirements,
        max_tokens: spec.max_tokens,
        admission: decision.admission,
        granted_tokens: decision.granted_tokens,
        blockers: decision.reasons.clone(),
        obligations: decision.obligations.clone(),
    })
}

impl TaskBrief {
    /// 渲染成确定性的纯文本提示；宿主可替换成自己的模板。
    pub fn render(&self) -> String {
        render_brief(self)
    }
}

pub fn render_brief(brief: &TaskBrief) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Task {} (epoch {})", brief.task, brief.epoch);
    let _ = writeln!(out);

    let _ = writeln!(out, "## Objective");
    if brief.guidance.objective.is_empty() {
        let _ = writeln!(out, "(not declared; host interprets)");
    } else {
        let _ = writeln!(out, "{}", brief.guidance.objective);
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Constraints");
    write_list(&mut out, &brief.guidance.constraints);
    let _ = writeln!(out);

    let _ = writeln!(out, "## Stop when");
    write_list(&mut out, &brief.guidance.stop_when);
    let _ = writeln!(out);

    let _ = writeln!(out, "## Evidence");
    if brief.resolves.is_empty() {
        let _ = writeln!(out, "- resolves: (none)");
    } else {
        let _ = writeln!(out, "- resolves: {}", brief.resolves.join(", "));
    }
    let _ = writeln!(out, "- min_assertions: {}", brief.produces.min_assertions);
    let _ = writeln!(out);

    let _ = writeln!(out, "## Effects");
    if brief.effects.is_empty() {
        let _ = writeln!(out, "- {} (task id fallback)", brief.task);
    } else {
        for effect in &brief.effects {
            let _ = writeln!(out, "- {}", effect.operation_for(&brief.task));
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Admission");
    let _ = writeln!(out, "- state: {:?}", brief.admission);
    let _ = writeln!(out, "- granted_tokens: {}", brief.granted_tokens);
    if brief.blockers.is_empty() {
        let _ = writeln!(out, "- blockers: (none)");
    } else {
        for reason in &brief.blockers {
            let _ = writeln!(out, "- blocker: {}", render_blocker(reason));
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Open obligations");
    if brief.obligations.is_empty() {
        let _ = writeln!(out, "- (none)");
    } else {
        for obligation in &brief.obligations {
            let owner = obligation.owner.as_deref().unwrap_or("-");
            let human = if obligation.cost.human { ", human" } else { "" };
            let _ = writeln!(
                out,
                "- [{}] {} (owner {}, {} tokens{})",
                obligation.kind.as_str(),
                obligation.subject,
                owner,
                obligation.cost.tokens,
                human
            );
            if !obligation.detail.is_empty() {
                let _ = writeln!(out, "  {}", obligation.detail);
            }
        }
    }
    out
}

fn write_list(out: &mut String, items: &[String]) {
    if items.is_empty() {
        let _ = writeln!(out, "- (none)");
    } else {
        for item in items {
            let _ = writeln!(out, "- {item}");
        }
    }
}

fn render_blocker(reason: &BlockReason) -> String {
    match reason {
        BlockReason::Dependency {
            task,
            required,
            verdict,
        } => format!("dependency {task}: required={required:?}, verdict={verdict:?}"),
        BlockReason::Authorization { verdict } => format!("authorization: {verdict:?}"),
        BlockReason::Freshness { verdict } => format!("freshness: {verdict:?}"),
        BlockReason::Claim { verdict } => format!("claim: {verdict:?}"),
        BlockReason::Effect { state } => format!("effect: {state:?}"),
        BlockReason::Conflict { task } => format!("conflict on {task}"),
        BlockReason::ResourceCapacity {
            requirements,
            capacity,
        } => format!("resource capacity: needs {requirements:?}, capacity {capacity:?}"),
        BlockReason::Resources {
            requirements,
            available,
        } => format!("resources: needs {requirements:?}, available {available:?}"),
        BlockReason::Workers { occupied, limit } => format!("workers: {occupied}/{limit}"),
        BlockReason::TokenCapacity { required, limit } => {
            format!("token capacity: {required}/{limit}")
        }
        BlockReason::Tokens {
            required,
            available,
        } => format!("tokens: {required}/{available}"),
        BlockReason::InFlight => "in flight".into(),
        BlockReason::Settled => "settled".into(),
        BlockReason::Superseded {
            run_epoch,
            task_epoch,
        } => format!("superseded: run epoch {run_epoch}, task epoch {task_epoch}"),
    }
}
