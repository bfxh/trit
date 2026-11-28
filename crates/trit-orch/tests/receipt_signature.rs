use std::collections::BTreeMap;

use trit_orch::{
    Error, EvidenceSpec, Exit, Phase, Progress, Receipt, TaskSpec, Ternary, TokenUsage,
    acceptance_digest,
};

#[path = "progress_support/mod.rs"]
mod progress_support;
use progress_support::*;

const KEY: &[u8] = b"0123456789abcdef";

fn settled() -> ([TaskSpec; 1], trit_orch::Snapshot) {
    let tasks = [task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    let mut report = passed();
    report.checks = 2;
    a.evidence = Some(report);
    (tasks, state)
}

fn host_receipt(task: &TaskSpec, revision: u64, key: Option<&[u8]>) -> Receipt {
    let mut receipt = Receipt::host_measured(task.epoch, Exit::Completed { code: 0 }, 2, 0)
        .with_plan(
            revision,
            acceptance_digest(task, 1).expect("acceptance digest"),
        )
        .with_observation(Ternary::True, Ternary::True, TokenUsage::default());
    if let Some(key) = key {
        receipt.sign(key);
    }
    receipt
}

fn finish(receipt: Receipt, key: &[u8]) -> Result<trit_orch::Summary, Error> {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt);
    Progress::new().finish_with_receipts_hmac(
        &tasks,
        &state,
        &receipts,
        EvidenceSpec { min_assertions: 1 },
        None,
        key,
    )
}

#[test]
fn signed_host_receipt_passes_hmac_gate() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        host_receipt(&tasks[0], state.revision, Some(KEY)),
    );
    let summary = Progress::new()
        .finish_with_receipts_hmac(
            &tasks,
            &state,
            &receipts,
            EvidenceSpec { min_assertions: 1 },
            None,
            KEY,
        )
        .expect("valid signature");
    assert_eq!(summary.confirmed, 1);
}

#[test]
fn missing_signature_is_refused() {
    let (tasks, state) = settled();
    let receipt = host_receipt(&tasks[0], state.revision, None);
    assert_eq!(
        finish(receipt, KEY),
        Err(Error::ReceiptObservationMissing {
            task: "a".to_owned(),
            field: "signature",
        })
    );
}

#[test]
fn wrong_key_or_tampering_is_refused() {
    let (tasks, state) = settled();
    let wrong = host_receipt(&tasks[0], state.revision, Some(b"fedcba9876543210"));
    assert_eq!(
        finish(wrong, KEY),
        Err(Error::ReceiptObservationMismatch {
            task: "a".to_owned(),
            field: "signature",
        })
    );
    let mut tampered = host_receipt(&tasks[0], state.revision, Some(KEY));
    tampered.nonce = "changed-after-signing".to_owned();
    assert_eq!(
        finish(tampered, KEY),
        Err(Error::ReceiptObservationMismatch {
            task: "a".to_owned(),
            field: "signature",
        })
    );
}

#[test]
fn unsigned_legacy_gate_still_accepts_unsigned_receipts() {
    let (tasks, state) = settled();
    let mut receipts = BTreeMap::new();
    receipts.insert(
        "a".to_owned(),
        host_receipt(&tasks[0], state.revision, None),
    );
    let summary = Progress::new()
        .finish_with_receipts(
            &tasks,
            &state,
            &receipts,
            EvidenceSpec { min_assertions: 1 },
        )
        .expect("legacy unsigned path");
    assert_eq!(summary.confirmed, 1);
}
