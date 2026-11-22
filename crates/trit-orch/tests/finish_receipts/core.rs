use std::collections::BTreeMap;

use trit_orch::{Error, EvidenceSpec, Exit, Phase, Progress, Receipt, Ternary, TokenUsage};

#[path = "../progress_support/mod.rs"]
mod progress_support;
use progress_support::*;

fn host_receipt(passed: u32, failed: u32) -> Receipt {
    Receipt::host_measured(0, Exit::Completed { code: 0 }, passed, failed).with_observation(
        Ternary::True,
        Ternary::True,
        TokenUsage::default(),
    )
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
    receipts.insert("a".to_owned(), host_receipt(1, 0));
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
    mismatch.insert("a".to_owned(), host_receipt(2, 0));
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
    receipts.insert("a".to_owned(), host_receipt(0, 1));
    assert_eq!(
        Progress::new().finish_with_receipts(&tasks, &state, &receipts, spec()),
        Err(Error::ReceiptMismatch {
            task: "a".to_owned()
        }),
        "ORCH_FINISH_RECEIPT_CONTRADICTS"
    );
}

#[test]
fn self_reported_receipt_is_refused_by_default() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        Receipt::self_reported(0, Exit::Completed { code: 0 }, 1, 0).with_observation(
            Ternary::True,
            Ternary::True,
            TokenUsage::default(),
        ),
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

#[test]
fn host_nonce_binding_rejects_replayed_receipt() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        host_receipt(1, 0).with_audit("run-2", vec!["cargo".into(), "test".into()]),
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
