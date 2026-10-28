use serde::{Deserialize, Serialize};

use crate::{Effect, Ledger, Limits, Plan, Selection, Snapshot, TaskSpec, policy::ShadowPolicy};

/// 一步回放输入：当时的计划、快照与资源上限。内核决定在回放时重算，不信历史结论。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceStep {
    pub revision: u64,
    pub tasks: Vec<TaskSpec>,
    pub snapshot: Snapshot,
    pub limits: Limits,
    #[serde(default)]
    pub ledger: Ledger,
}

/// 影子回放报告。只统计“建议”与内核决定的差，**不产生任何派发动作**。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowReport {
    pub policy: String,
    pub steps: usize,
    /// 建议额外派发、但内核没选：日志 `step:task`。
    pub rejected: Vec<String>,
    /// 其中属于「效果可能已发生」的危险建议，任何一条都该判红。
    pub unsafe_extra: Vec<String>,
    /// 建议跳过、但内核选了：会漏派。
    pub skipped: Vec<String>,
    pub order_mismatches: usize,
    pub stop_requests: usize,
    /// 与内核完全一致的步数。
    pub agreed_steps: usize,
}

impl ShadowReport {
    /// 安全红线：影子策略不得建议重发效果可能已发生的操作。
    pub fn is_safe(&self) -> bool {
        self.unsafe_extra.is_empty()
    }
}

/// 回放一段轨迹。每一步都重新执行内核的 `select`，策略只读内核结果。
pub fn replay(
    policy: &dyn ShadowPolicy,
    steps: &[TraceStep],
) -> Result<ShadowReport, crate::Error> {
    let mut report = ShadowReport {
        policy: policy.name().to_owned(),
        steps: steps.len(),
        ..ShadowReport::default()
    };
    for (index, step) in steps.iter().enumerate() {
        let plan = Plan::new(step.revision, step.tasks.clone())?;
        let selection = plan.select(&step.snapshot, &step.limits)?;
        let context = crate::policy::ShadowContext {
            selection: &selection,
            tasks: &step.tasks,
            ledger: &step.ledger,
            limits: &step.limits,
        };
        let proposal = policy.propose(&context);
        let step_agreed = accumulate(&mut report, index, &selection, &proposal, &step.ledger);
        if step_agreed {
            report.agreed_steps += 1;
        }
    }
    Ok(report)
}

fn accumulate(
    report: &mut ShadowReport,
    index: usize,
    selection: &Selection,
    proposal: &crate::policy::Proposal,
    ledger: &Ledger,
) -> bool {
    let selected: std::collections::BTreeSet<&str> =
        selection.selected.iter().map(String::as_str).collect();
    let mut agreed = true;

    for id in &proposal.extra {
        if selected.contains(id.as_str()) {
            continue;
        }
        agreed = false;
        let entry = format!("{index}:{id}");
        if matches!(
            ledger.state(id),
            Some(Effect::Sending | Effect::Sent | Effect::Unknown | Effect::Confirmed)
        ) {
            report.unsafe_extra.push(entry.clone());
        }
        report.rejected.push(entry);
    }

    if let Some(order) = &proposal.order {
        let ordered: Vec<&str> = order.iter().map(String::as_str).collect();
        let kernel: Vec<&str> = selection.selected.iter().map(String::as_str).collect();
        if ordered != kernel {
            report.order_mismatches += 1;
            agreed = false;
        }
        if order.len() != selection.selected.len()
            || order.iter().any(|id| !selected.contains(id.as_str()))
        {
            // 顺序建议不能偷偷改变集合。
            report.unsafe_extra.push(format!("{index}:order-set"));
            agreed = false;
        }
    }

    for id in &proposal.skip {
        if selected.contains(id.as_str()) {
            agreed = false;
            report.skipped.push(format!("{index}:{id}"));
        }
    }

    if proposal.stop {
        report.stop_requests += 1;
        agreed = false;
    }

    agreed
}
