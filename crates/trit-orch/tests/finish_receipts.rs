use std::collections::BTreeMap;

use trit_orch::{Error, EvidenceSpec, Exit, Phase, Progress, Receipt};

mod progress_support;
use progress_support::*;

fn receipt(passed: u32, failed: u32) -> Receipt {
    Receipt {
        epoch: 0,
        exit: Exit::Completed { code: 0 },
        assertions_passed: passed,
        assertions_failed: failed,
    }
}

#[test]
fn finish_with_receipts_requires_matching_executor_receipt() {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    a.evidence = Some(passed());
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt(1, 0));
    let summary = Progress::new()
        .finish_with_receipts(
            &tasks,
            &state,
            &receipts,
            EvidenceSpec { min_assertions: 1 },
        )
        .unwrap();
    assert_eq!(summary.confirmed, 1);

    let missing = Progress::new().finish_with_receipts(
        &tasks,
        &state,
        &BTreeMap::new(),
        EvidenceSpec { min_assertions: 1 },
    );
    assert_eq!(
        missing,
        Err(Error::UnattestedEvidence {
            count: 1,
            tasks: vec!["a".to_owned()]
        }),
        "ORCH_FINISH_UNATTESTED"
    );

    let mut mismatch = BTreeMap::new();
    mismatch.insert("a".to_owned(), receipt(2, 0));
    assert_eq!(
        Progress::new().finish_with_receipts(
            &tasks,
            &state,
            &mismatch,
            EvidenceSpec { min_assertions: 1 },
        ),
        Err(Error::ReceiptMismatch {
            task: "a".to_owned()
        }),
        "ORCH_FINISH_RECEIPT_MISMATCH"
    );
}

#[test]
fn finish_with_receipts_rejects_failed_receipt_for_passed_claim() {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    a.evidence = Some(passed());
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt(0, 1));
    assert_eq!(
        Progress::new().finish_with_receipts(
            &tasks,
            &state,
            &receipts,
            EvidenceSpec { min_assertions: 1 },
        ),
        Err(Error::ReceiptMismatch {
            task: "a".to_owned()
        }),
        "ORCH_FINISH_RECEIPT_CONTRADICTS"
    );
}
