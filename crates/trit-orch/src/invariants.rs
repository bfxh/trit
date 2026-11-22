//! 判定层与完成闸门的不变量检查器。
//!
//! 把设计文档里那三条「边界不变量」从口头原则变成可机器判红的函数：
//!
//! - **`unknown_without_obligation`**：准入是 `Unknown` 就必须有补证义务，否则未知成了死路。
//! - **`conflict_without_adjudication`**：有 `Conflict` 原因就必须有 `Adjudicate` 义务（且指向同一个冲突源），
//!   否则冲突会被"再测一次"或多数投票悄悄压掉。
//! - **`finish_with_undetermined`**：声称完成时，快照里不允许还有 pending / in_flight / 未定 / 冲突。
//!
//! 另外三条结构不变量同源，一起查：判定已确定却带义务（补证掩盖拒单）、义务 ID 偏离规范
//! （会破坏去重与审计）、同一判定里同 `(kind, subject)` 重复。
//!
//! 这个检查器**不复用闸门自己的结论**：`finish_with_undetermined` 从快照重新数，而不是读
//! `Progress::rounds`。闸门自己算错时，读它的输出是查不出来的。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::obligation::canonical_id;
use crate::progress::decisive;
use crate::{
    BlockReason, Decision, EvidenceVerdict, ObligationKind, Phase, Selection, Snapshot, TaskSpec,
    Ternary,
};

/// 不变量名。取值同时是 CI 日志里的稳定字符串，改名等于破坏对外契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Invariant {
    /// `Unknown` 准入没有产生任何义务。
    UnknownWithoutObligation,
    /// 有冲突原因，却没有指向该冲突的裁决义务。
    ConflictWithoutAdjudication,
    /// 判定已确定（`True`/`False`）却挂着义务。
    ObligationOnDecisive,
    /// 义务 ID 不是 `kind:task:subject`。
    ObligationIdDrift,
    /// 同一判定里出现重复的 `(kind, subject)`。
    DuplicateObligation,
    /// 任务被选中，但它的判定不是 `True`。
    SelectedWithoutAdmission,
    /// 判定是 `True` 却没被选中（预算是已扣的）。
    AdmissionWithoutSelection,
    /// 声称完成时仍有未定/在途/冲突。
    FinishWithUndetermined,
}

impl Invariant {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownWithoutObligation => "unknown_without_obligation",
            Self::ConflictWithoutAdjudication => "conflict_without_adjudication",
            Self::ObligationOnDecisive => "obligation_on_decisive",
            Self::ObligationIdDrift => "obligation_id_drift",
            Self::DuplicateObligation => "duplicate_obligation",
            Self::SelectedWithoutAdmission => "selected_without_admission",
            Self::AdmissionWithoutSelection => "admission_without_selection",
            Self::FinishWithUndetermined => "finish_with_undetermined",
        }
    }

    /// 全部不变量，按声明顺序。CI 可以据此断言"一条都没漏"。
    pub const ALL: [Invariant; 8] = [
        Self::UnknownWithoutObligation,
        Self::ConflictWithoutAdjudication,
        Self::ObligationOnDecisive,
        Self::ObligationIdDrift,
        Self::DuplicateObligation,
        Self::SelectedWithoutAdmission,
        Self::AdmissionWithoutSelection,
        Self::FinishWithUndetermined,
    ];
}

/// 一条违例。带 `task` / `subject` 而不是只给一句话，是为了能直接定位到决定与义务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub invariant: Invariant,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub detail: String,
}

impl Violation {
    fn at(invariant: Invariant, task: &str, detail: impl Into<String>) -> Self {
        Self {
            invariant,
            task: Some(task.to_owned()),
            subject: None,
            detail: detail.into(),
        }
    }

    fn on(invariant: Invariant, task: &str, subject: &str, detail: impl Into<String>) -> Self {
        Self {
            invariant,
            task: Some(task.to_owned()),
            subject: Some(subject.to_owned()),
            detail: detail.into(),
        }
    }

    fn global(invariant: Invariant, detail: impl Into<String>) -> Self {
        Self {
            invariant,
            task: None,
            subject: None,
            detail: detail.into(),
        }
    }

    /// 一行可读证书：`invariant task=… subject=… detail`。
    pub fn brief(&self) -> String {
        let mut line = self.invariant.as_str().to_owned();
        if let Some(task) = &self.task {
            line.push_str(" task=");
            line.push_str(task);
        }
        if let Some(subject) = &self.subject {
            line.push_str(" subject=");
            line.push_str(subject);
        }
        line.push_str(" detail=");
        line.push_str(&self.detail);
        line
    }
}

/// 一次检查的结果。空 = 干净。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvariantReport {
    pub violations: Vec<Violation>,
}

impl InvariantReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.violations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn merge(&mut self, other: InvariantReport) {
        self.violations.extend(other.violations);
    }

    /// 按不变量分组计数，供面板与 CI 摘要。
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for violation in &self.violations {
            *counts.entry(violation.invariant.as_str()).or_insert(0) += 1;
        }
        counts
    }

    /// CI 入口：干净则 `Ok(())`，否则把报告原样返回（保留全部证书，不只第一条）。
    pub fn assert_clean(self) -> Result<(), InvariantReport> {
        if self.is_clean() { Ok(()) } else { Err(self) }
    }

    pub fn brief(&self) -> String {
        if self.is_clean() {
            return "invariants: clean".to_owned();
        }
        let counts = self
            .counts()
            .into_iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("invariants: {} violated [{counts}]", self.violations.len())
    }
}

/// 状态类判定：在途 / 已结算的决定没走准入，`Unknown` 是初始值而不是"待补证"。
fn is_status_only(decision: &Decision) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| matches!(reason, BlockReason::InFlight | BlockReason::Settled))
}

/// 检查一次选择结果。
pub fn check_selection(selection: &Selection) -> InvariantReport {
    let mut report = InvariantReport::default();
    let mut selected: BTreeSet<&str> = BTreeSet::new();
    selected.extend(selection.selected.iter().map(String::as_str));

    for decision in &selection.decisions {
        let task = decision.task.as_str();
        let status_only = is_status_only(decision);

        // 1. Unknown 必须有义务（状态类判定除外）。
        if !status_only && decision.admission == Ternary::Unknown && decision.obligations.is_empty()
        {
            report.violations.push(Violation::at(
                Invariant::UnknownWithoutObligation,
                task,
                "准入未知但没有补证义务；未知成了死路",
            ));
        }

        // 2. 冲突必须有指向同一冲突源的裁决义务。
        for reason in &decision.reasons {
            let BlockReason::Conflict { task: subject } = reason else {
                continue;
            };
            let adjudicated = decision.obligations.iter().any(|obligation| {
                obligation.kind == ObligationKind::Adjudicate
                    && obligation.subject == *subject
                    && obligation.task == task
            });
            if !adjudicated {
                report.violations.push(Violation::on(
                    Invariant::ConflictWithoutAdjudication,
                    task,
                    subject,
                    "证据冲突没有裁决义务；冲突会被再测或多数投票压掉",
                ));
            }
        }

        // 3. 已确定却带义务：补证不该挂在 True/False 上。
        if decision.admission != Ternary::Unknown && !decision.obligations.is_empty() {
            report.violations.push(Violation::at(
                Invariant::ObligationOnDecisive,
                task,
                format!(
                    "准入是 {:?} 却挂着 {} 条义务",
                    decision.admission,
                    decision.obligations.len()
                ),
            ));
        }

        // 4/5. 义务自身的形状。
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for obligation in &decision.obligations {
            let expected = canonical_id(obligation.kind, &obligation.task, &obligation.subject);
            if obligation.id != expected {
                report.violations.push(Violation::on(
                    Invariant::ObligationIdDrift,
                    task,
                    &obligation.subject,
                    format!("ID 是 `{}`，规范是 `{expected}`", obligation.id),
                ));
            }
            if !seen.insert(format!(
                "{}:{}",
                obligation.kind.as_str(),
                obligation.subject
            )) {
                report.violations.push(Violation::on(
                    Invariant::DuplicateObligation,
                    task,
                    &obligation.subject,
                    format!("`{}` 重复出现", obligation.kind.as_str()),
                ));
            }
        }
    }

    // 6/7. 选中与准入必须互为充要（预算只在 `True` 时扣）。
    for decision in &selection.decisions {
        let task = decision.task.as_str();
        let is_selected = selected.contains(task);
        if !is_status_only(decision) {
            if is_selected && decision.admission != Ternary::True {
                report.violations.push(Violation::at(
                    Invariant::SelectedWithoutAdmission,
                    task,
                    format!("被选中但准入是 {:?}", decision.admission),
                ));
            }
            if !is_selected && decision.admission == Ternary::True {
                report.violations.push(Violation::at(
                    Invariant::AdmissionWithoutSelection,
                    task,
                    "准入是 True 却没被选中；预算已扣但不会派发",
                ));
            }
        }
    }

    // 选中的任务必须真的在决定列表里。
    for id in &selection.selected {
        if selection
            .decisions
            .binary_search_by(|decision| decision.task.as_str().cmp(id.as_str()))
            .is_err()
            && !selection
                .decisions
                .iter()
                .any(|decision| decision.task == *id)
        {
            report.violations.push(Violation::at(
                Invariant::SelectedWithoutAdmission,
                id,
                "被选中但不在决定列表里",
            ));
        }
    }

    report
}

/// 检查「声称完成」：从快照重新数，不信 `Progress` 自己的结论。
pub fn check_finish_claim(tasks: &[TaskSpec], snapshot: &Snapshot) -> InvariantReport {
    let mut pending = 0usize;
    let mut in_flight = 0usize;
    let mut inconclusive = 0usize;
    let mut conflicts = 0usize;
    for spec in tasks {
        let Some(observation) = snapshot.observations.get(&spec.task.id) else {
            pending += 1;
            continue;
        };
        match observation.phase {
            Phase::Pending => pending += 1,
            Phase::InFlight => in_flight += 1,
            Phase::Settled => match decisive(observation, spec) {
                Some(EvidenceVerdict::Proved) | Some(EvidenceVerdict::Refuted) => {}
                Some(EvidenceVerdict::Conflicted) => conflicts += 1,
                _ => inconclusive += 1,
            },
        }
    }
    let mut report = InvariantReport::default();
    if pending + in_flight + inconclusive + conflicts > 0 {
        report.violations.push(Violation::global(
            Invariant::FinishWithUndetermined,
            format!(
                "pending={pending} in_flight={in_flight} inconclusive={inconclusive} conflicts={conflicts}"
            ),
        ));
    }
    report
}

/// 一次跑完全部检查：判定层 + 完成闸门。
pub fn check_invariants(
    selection: &Selection,
    tasks: &[TaskSpec],
    snapshot: &Snapshot,
) -> InvariantReport {
    let mut report = check_selection(selection);
    report.merge(check_finish_claim(tasks, snapshot));
    report
}
