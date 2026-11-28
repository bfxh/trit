use std::collections::BTreeMap;

use trit_orch::{
    Error, EvidenceSpec, Exit, Phase, Progress, Receipt, TaskSpec, Ternary, TokenUsage,
    acceptance_digest,
};

#[path = "progress_support/mod.rs"]
mod progress_support;
use progress_support::*;

fn spec(min_assertions: u32) -> EvidenceSpec {
    EvidenceSpec { min_assertions }
}

fn settled(checks: u32) -> ([TaskSpec; 1], trit_orch::Snapshot) {
    let tasks = [task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    let mut report = passed();
    report.checks = checks;
    a.evidence = Some(report);
    (tasks, state)
}

fn receipt(task: &TaskSpec, revision: u64, min_assertions: u32, passed: u32) -> Receipt {
    Receipt::host_measured(task.epoch, Exit::Completed { code: 0 }, passed, 0)
        .with_plan(
            revision,
            acceptance_digest(task, min_assertions).expect("acceptance digest"),
        )
        .with_observation(Ternary::True, Ternary::True, TokenUsage::default())
}

#[test]
fn task_floor_raises_above_global_floor() {
    let (mut tasks, state) = settled(3);
    tasks[0].policy.min_assertions = 3;
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt(&tasks[0], state.revision, 3, 3));
    // 全局只给 1，任务自己要求 3；有效阈值必须是 3。
    let summary = Progress::new()
        .finish_with_receipts(&tasks, &state, &receipts, spec(1))
        .expect("task floor is authoritative");
    assert_eq!(summary.confirmed, 1);
}

#[test]
fn global_floor_can_raise_but_not_lower_task_floor() {
    let (mut tasks, state) = settled(3);
    tasks[0].policy.min_assertions = 3;
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt(&tasks[0], state.revision, 3, 3));
    // 全局 4 抬高有效阈值；回执仍按 3 绑定，必须拒绝。
    assert_eq!(
        Progress::new().finish_with_receipts(&tasks, &state, &receipts, spec(4)),
        Err(Error::ReceiptObservationMismatch {
            task: "a".to_owned(),
            field: "acceptance_digest",
        }),
        "ORCH_FINISH_GLOBAL_FLOOR_RAISES"
    );
    // 全局 0 不能把任务下限压到 0。
    let summary = Progress::new()
        .finish_with_receipts(&tasks, &state, &receipts, spec(0))
        .expect("global zero cannot lower task floor");
    assert_eq!(summary.confirmed, 1);
}

#[test]
fn legacy_task_json_without_policy_still_gets_min_one() {
    let tasks = [task("a")];
    let mut value = serde_json::to_value(&tasks[0]).expect("serialize task");
    let object = value.as_object_mut().expect("task object");
    object.remove("min_assertions");
    let parsed: TaskSpec = serde_json::from_value(value).expect("legacy task json");
    assert_eq!(parsed.epoch, 0);
    assert_eq!(parsed.policy.min_assertions, 1);
    let round_trip = serde_json::to_value(&parsed).expect("reserialize task");
    assert_eq!(round_trip["epoch"], 0);
    assert_eq!(round_trip["min_assertions"], 1);
}
