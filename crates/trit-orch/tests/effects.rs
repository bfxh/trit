use trit_orch::{Attempt, Effect, Error, Ledger, Reconcile};

fn prepared(ledger: &mut Ledger, op: &str, fence: u64) -> Attempt {
    ledger.prepare("task-a", 1, op, fence).unwrap()
}

#[test]
fn fresh_operation_starts_prepared_and_retryable() {
    let ledger = Ledger::new();
    assert_eq!(ledger.state("pay"), None);
    assert!(ledger.may_retry("pay"));
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "pay", 1);
    assert_eq!(attempt.attempt, 1);
    assert_eq!(ledger.state("pay"), Some(Effect::Prepared));
    assert!(!ledger.may_retry("pay"), "ORCH_EFFECT_NO_BLIND_RETRY");
    assert_eq!(ledger.len(), 1);
}

#[test]
fn in_doubt_effect_cannot_be_re_prepared() {
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "pay", 1);
    ledger.mark_sent(&attempt, 1).unwrap();
    ledger.mark_unknown(&attempt).unwrap();
    assert_eq!(ledger.state("pay"), Some(Effect::Unknown));
    assert!(!ledger.may_retry("pay"), "ORCH_EFFECT_NO_BLIND_RETRY");
    assert_eq!(
        ledger.prepare("task-a", 1, "pay", 2),
        Err(Error::EffectInDoubt {
            operation: "pay".into()
        }),
        "ORCH_EFFECT_NO_BLIND_RETRY"
    );
}

#[test]
fn reconcile_absent_marks_refuted_then_retry_is_allowed() {
    let mut ledger = Ledger::new();
    let first = prepared(&mut ledger, "pay", 1);
    ledger.mark_sent(&first, 1).unwrap();
    ledger.mark_unknown(&first).unwrap();
    assert_eq!(
        ledger.reconcile(&first, Reconcile::Absent).unwrap(),
        Effect::Refuted,
        "ORCH_EFFECT_RECONCILE"
    );
    assert!(ledger.may_retry("pay"));
    let second = ledger.prepare("task-a", 1, "pay", 2).unwrap();
    assert_eq!(second.attempt, 2);
    assert_eq!(ledger.state("pay"), Some(Effect::Prepared));
}

#[test]
fn reconcile_present_confirms_and_blocks_further_attempts() {
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "pay", 1);
    ledger.mark_sent(&attempt, 1).unwrap();
    assert_eq!(
        ledger.reconcile(&attempt, Reconcile::Present).unwrap(),
        Effect::Confirmed,
        "ORCH_EFFECT_RECONCILE"
    );
    assert!(!ledger.may_retry("pay"), "ORCH_EFFECT_RECONCILE");
    assert_eq!(
        ledger.prepare("task-a", 1, "pay", 2),
        Err(Error::EffectAlreadyApplied {
            operation: "pay".into()
        }),
        "ORCH_EFFECT_TERMINAL"
    );
    assert_eq!(
        ledger.reconcile(&attempt, Reconcile::Absent),
        Err(Error::InvalidTransition {
            operation: "pay".into(),
            from: "Confirmed".into(),
            to: "reconcile Absent".into()
        })
    );
}

#[test]
fn definitive_failure_refutes_and_allows_new_attempt() {
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "pay", 1);
    ledger.mark_sent(&attempt, 1).unwrap();
    ledger.record_result(&attempt, false).unwrap();
    assert_eq!(ledger.state("pay"), Some(Effect::Refuted));
    let next = ledger.prepare("task-a", 1, "pay", 1).unwrap();
    assert_eq!(next.attempt, 2);
}

#[test]
fn stale_fence_cannot_send_or_prepare_over_newer_owner() {
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "pay", 5);
    assert_eq!(
        ledger.mark_sent(&attempt, 4),
        Err(Error::StaleFence {
            operation: "pay".into(),
            given: 4,
            current: 5
        }),
        "ORCH_EFFECT_FENCE"
    );
    ledger.mark_sent(&attempt, 5).unwrap();
    ledger.record_result(&attempt, false).unwrap();
    assert_eq!(
        ledger.prepare("task-a", 1, "pay", 3),
        Err(Error::StaleFence {
            operation: "pay".into(),
            given: 3,
            current: 5
        }),
        "ORCH_EFFECT_FENCE"
    );
    assert!(ledger.prepare("task-a", 1, "pay", 6).is_ok());
}

#[test]
fn attempt_identity_must_match() {
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "pay", 1);
    let mut forged = attempt.clone();
    forged.attempt = 2;
    assert_eq!(
        ledger.mark_sent(&forged, 1),
        Err(Error::AttemptMismatch {
            operation: "pay".into(),
            given: 2,
            current: 1
        })
    );
    assert_eq!(ledger.mark_sent(&attempt, 1).err(), None);
    assert_eq!(ledger.state("pay"), Some(Effect::Sent));
}

#[test]
fn sent_cannot_skip_unknown_or_reconcile_by_prepare() {
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "pay", 1);
    ledger.mark_sent(&attempt, 1).unwrap();
    assert_eq!(
        ledger.mark_sent(&attempt, 1),
        Err(Error::InvalidTransition {
            operation: "pay".into(),
            from: "Sent".into(),
            to: "Sent".into()
        })
    );
    assert_eq!(
        ledger.prepare("task-a", 1, "pay", 2),
        Err(Error::EffectInDoubt {
            operation: "pay".into()
        })
    );
    ledger.reconcile(&attempt, Reconcile::Absent).unwrap();
    assert!(ledger.may_retry("pay"));
}

#[test]
fn unknown_operation_is_reported_not_guessed() {
    let mut ledger = Ledger::new();
    let ghost = Attempt {
        operation: "ghost".into(),
        task: "task-a".into(),
        epoch: 1,
        attempt: 1,
        fence: 1,
    };
    assert_eq!(
        ledger.mark_sent(&ghost, 1),
        Err(Error::UnknownOperation {
            operation: "ghost".into()
        })
    );
    assert_eq!(
        ledger.mark_unknown(&ghost),
        Err(Error::UnknownOperation {
            operation: "ghost".into()
        })
    );
}
