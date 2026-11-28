use std::collections::BTreeMap;

use trit_orch::{
    Error, EvidenceSpec, Exit, Phase, Progress, Receipt, TaskSpec, Ternary, TokenUsage,
    task_spec_digest,
};

#[path = "../progress_support/mod.rs"]
mod progress_support;
use progress_support::*;

fn host_receipt(passed: u32, failed: u32, task: &TaskSpec, revision: u64) -> Receipt {
    Receipt::host_measured(task.epoch, Exit::Completed { code: 0 }, passed, failed)
        .with_plan(revision, task_spec_digest(task).expect("task digest"))
        .with_observation(Ternary::True, Ternary::True, TokenUsage::default())
}

fn spec() -> EvidenceSpec {
    EvidenceSpec { min_assertions: 1 }
}

fn settled() -> (Vec<trit_orch::TaskSpec>, trit_orch::Snapshot) {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    a.evidence = Some(passed());
    (tasks, state)
}

#[test]
fn finish_with_receipts_requires_matching_executor_receipt() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        host_receipt(1, 0, &tasks[0], state.revision),
    );
    let summary = Progress::new()
        .finish_with_receipts(&tasks, &state, &receipts, spec())
        .unwrap();
    assert_eq!(summary.confirmed, 1);

    let missing = Progress::new().finish_with_receipts(&tasks, &state, &BTreeMap::new(), spec());
    assert_eq!(
        missing,
        Err(Error::UnattestedEvidence {
            count: 1,
            tasks: vec!["a".to_owned()]
        }),
        "ORCH_FINISH_UNATTESTED"
    );

    let mut mismatch = BTreeMap::new();
    mismatch.insert(
        "a".to_owned(),
        host_receipt(2, 0, &tasks[0], state.revision),
    );
    assert_eq!(
        Progress::new().finish_with_receipts(&tasks, &state, &mismatch, spec()),
        Err(Error::ReceiptMismatch {
            task: "a".to_owned()
        }),
        "ORCH_FINISH_RECEIPT_MISMATCH"
    );
}

#[test]
fn finish_with_receipts_rejects_failed_receipt_for_passed_claim() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        host_receipt(0, 1, &tasks[0], state.revision),
    );
    assert_eq!(
        Progress::new().finish_with_receipts(&tasks, &state, &receipts, spec()),
        Err(Error::ReceiptMismatch {
            task: "a".to_owned()
        }),
        "ORCH_FINISH_RECEIPT_CONTRADICTS"
    );
}

#[test]
fn host_nonce_binding_rejects_replayed_receipt() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        host_receipt(1, 0, &tasks[0], state.revision)
            .with_audit("run-2", vec!["cargo".into(), "test".into()]),
    );
    assert_eq!(
        Progress::new().finish_with_receipts_for_nonce(&tasks, &state, &receipts, spec(), "run-1"),
        Err(Error::ReceiptNonceMismatch {
            task: "a".to_owned()
        }),
        "ORCH_FINISH_REPLAYED_RECEIPT"
    );
    assert_eq!(
        Progress::new()
            .finish_with_receipts_for_nonce(&tasks, &state, &receipts, spec(), "run-2")
            .unwrap()
            .confirmed,
        1
    );
}
