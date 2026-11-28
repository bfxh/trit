use std::collections::BTreeMap;

use trit_orch::{
    Error, EvidenceSpec, Exit, Phase, Progress, Receipt, TaskSpec, Ternary, TokenUsage,
    acceptance_digest,
};

#[path = "../progress_support/mod.rs"]
mod progress_support;
use progress_support::*;

fn spec(min_assertions: u32) -> EvidenceSpec {
    EvidenceSpec { min_assertions }
}

fn settled(checks: u32) -> (Vec<TaskSpec>, trit_orch::Snapshot) {
    let tasks = vec![task("a")];
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
fn receipt_measured_with_other_threshold_is_refused() {
    let (tasks, state) = settled(2);
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt(&tasks[0], state.revision, 2, 2));
    assert_eq!(
        Progress::new().finish_with_receipts(&tasks, &state, &receipts, spec(1)),
        Err(Error::ReceiptObservationMismatch {
            task: "a".to_owned(),
            field: "acceptance_digest",
        }),
        "ORCH_FINISH_ACCEPTANCE_THRESHOLD"
    );
}

#[test]
fn receipt_and_threshold_must_both_match() {
    let (tasks, state) = settled(2);
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt(&tasks[0], state.revision, 2, 2));
    let summary = Progress::new()
        .finish_with_receipts(&tasks, &state, &receipts, spec(2))
        .expect("matching threshold");
    assert_eq!(summary.confirmed, 1);
}
