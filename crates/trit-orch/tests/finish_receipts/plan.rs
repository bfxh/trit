use std::collections::BTreeMap;

use trit_orch::{
    Error, EvidenceSpec, Exit, Phase, Progress, Receipt, TaskSpec, Ternary, TokenUsage,
    acceptance_digest,
};

#[path = "../progress_support/mod.rs"]
mod progress_support;
use progress_support::*;

fn host_receipt(passed: u32, failed: u32, task: &TaskSpec, revision: u64) -> Receipt {
    Receipt::host_measured(task.epoch, Exit::Completed { code: 0 }, passed, failed)
        .with_plan(revision, acceptance_digest(task, 1).expect("digest"))
        .with_observation(Ternary::True, Ternary::True, TokenUsage::default())
}

fn spec() -> EvidenceSpec {
    EvidenceSpec { min_assertions: 1 }
}

fn settled() -> (Vec<TaskSpec>, trit_orch::Snapshot) {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    a.evidence = Some(passed());
    (tasks, state)
}

#[test]
fn receipt_without_plan_binding_is_refused() {
    let (tasks, state) = settled();
    let receipt = Receipt::host_measured(tasks[0].epoch, Exit::Completed { code: 0 }, 1, 0)
        .with_observation(Ternary::True, Ternary::True, TokenUsage::default());
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt);
    assert_eq!(
        Progress::new().finish_with_receipts(&tasks, &state, &receipts, spec()),
        Err(Error::ReceiptObservationMismatch {
            task: "a".to_owned(),
            field: "plan",
        }),
        "ORCH_FINISH_PLAN_UNBOUND"
    );
}

#[test]
fn receipt_from_another_revision_is_refused() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        host_receipt(1, 0, &tasks[0], state.revision + 1),
    );
    assert_eq!(
        Progress::new().finish_with_receipts(&tasks, &state, &receipts, spec()),
        Err(Error::ReceiptObservationMismatch {
            task: "a".to_owned(),
            field: "revision",
        }),
        "ORCH_FINISH_PLAN_REVISION"
    );
}

#[test]
fn changed_task_definition_invalidates_the_receipt() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        host_receipt(1, 0, &tasks[0], state.revision),
    );

    let mut changed = tasks.clone();
    changed[0].max_tokens += 1;
    assert_eq!(
        Progress::new().finish_with_receipts(&changed, &state, &receipts, spec()),
        Err(Error::ReceiptObservationMismatch {
            task: "a".to_owned(),
            field: "acceptance_digest",
        }),
        "ORCH_FINISH_PLAN_TASK_DIGEST"
    );
}
#[test]
fn self_reported_receipt_is_refused_by_default() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        Receipt::self_reported(0, Exit::Completed { code: 0 }, 1, 0)
            .with_plan(
                state.revision,
                acceptance_digest(&tasks[0], 1).expect("digest"),
            )
            .with_observation(Ternary::True, Ternary::True, TokenUsage::default()),
    );
    assert_eq!(
        Progress::new().finish_with_receipts(&tasks, &state, &receipts, spec()),
        Err(Error::ReceiptSelfReported {
            task: "a".to_owned()
        }),
        "ORCH_FINISH_SELF_REPORTED"
    );

    // 显式逃生舱才接受；它对应"外部已有独立信任锚"的场景。
    assert_eq!(
        Progress::new()
            .finish_with_self_reported_receipts(&tasks, &state, &receipts, spec())
            .unwrap()
            .confirmed,
        1
    );
}
