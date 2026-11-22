use std::collections::BTreeMap;

use trit_orch::{Error, EvidenceSpec, Exit, Phase, Progress, Receipt, Ternary};

mod progress_support;
use progress_support::*;

fn receipt(passed: u32, failed: u32) -> Receipt {
    Receipt::host_measured(0, Exit::Completed { code: 0 }, passed, failed)
}

#[test]
fn finish_with_receipts_rejects_unauthorized_settled_task() {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    a.authorization = Ternary::False;
    a.evidence = Some(passed());
    let mut receipts = BTreeMap::new();
    receipts.insert("a".to_owned(), receipt(1, 0));
    assert_eq!(
        Progress::new().finish_with_receipts(
            &tasks,
            &state,
            &receipts,
            EvidenceSpec { min_assertions: 1 },
        ),
        Err(Error::Incomplete {
            pending: 0,
            in_flight: 0,
            inconclusive: 1,
            conflicts: 0
        }),
        "ORCH_FINISH_UNAUTHORIZED"
    );
}
