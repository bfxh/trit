use trit_orch::{
    AdmissionObligation, FlipRates, ObligationBudget, ObligationGuard, ObligationKind,
    ObligationSkipReason, ScorePolicy, schedule_obligations_by_score_with_guard,
    schedule_obligations_with_guard,
};

#[test]
fn guard_blocks_thrashed_obligation_and_keeps_budget_for_others() {
    let measure = AdmissionObligation::new(ObligationKind::Measure, "m", "dep", "补证")
        .with_deadline(100)
        .with_max_attempts(2);
    let measure_id = measure.id.clone();
    let reconcile =
        AdmissionObligation::new(ObligationKind::Reconcile, "r", "pay", "对账").with_deadline(200);

    let mut guard = ObligationGuard::new();
    guard.record_attempt(&measure);
    guard.record_attempt(&measure);
    assert!(guard.is_thrashed(&measure));
    assert_eq!(guard.attempts(&measure_id), 2);

    let batch = schedule_obligations_with_guard(
        vec![measure, reconcile],
        50,
        &ObligationBudget {
            tokens: 10_000,
            human: 0,
        },
        &guard,
    );
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.scheduled[0].kind, ObligationKind::Reconcile);
    assert!(batch.skipped.iter().any(|skip| {
        skip.id == measure_id && skip.reason == ObligationSkipReason::Thrashed { attempts: 2 }
    }));
    assert_eq!(batch.tokens_used, 200, "被跳过的义务不能消耗预算");
}

#[test]
fn exhausted_obligations_promote_to_handoff_or_escalation() {
    let reconcile = AdmissionObligation::new(ObligationKind::Reconcile, "r", "pay", "对账")
        .with_max_attempts(2);
    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "m", "dep", "补证").with_max_attempts(1);
    let already_escalated =
        AdmissionObligation::new(ObligationKind::Escalate, "h", "auth", "人决策");

    let mut guard = ObligationGuard::new();
    guard.record_attempt(&reconcile);
    guard.record_attempt(&reconcile);
    guard.record_attempt(&measure);

    let handoff = guard.escalation_for(&reconcile).expect("对账超限应转交接");
    assert_eq!(handoff.kind, ObligationKind::Handoff);
    assert_eq!(handoff.max_attempts, 1);
    assert_eq!(handoff.priority, 100);

    let escalated = guard.escalation_for(&measure).expect("补证超限应升级");
    assert_eq!(escalated.kind, ObligationKind::Escalate);
    assert_eq!(escalated.max_attempts, 1);

    assert!(
        guard.escalation_for(&already_escalated).is_none(),
        "Escalate 本身是终点，不能自我升级"
    );
}

#[test]
fn fulfilled_attempt_resets_thrash_counter() {
    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "m", "dep", "补证").with_max_attempts(1);
    let mut guard = ObligationGuard::new();
    guard.record_attempt(&measure);
    assert!(guard.is_thrashed(&measure));

    guard.record_fulfilled(&measure);
    assert_eq!(guard.attempts(&measure.id), 0);
    assert!(!guard.is_thrashed(&measure));
}

#[test]
fn score_scheduler_respects_guard() {
    let mut rates = FlipRates::default();
    for _ in 0..3 {
        rates.record(ObligationKind::Measure, true);
    }
    for _ in 0..3 {
        rates.record(ObligationKind::Reconcile, false);
    }

    let measure = AdmissionObligation::new(ObligationKind::Measure, "m", "dep", "补证")
        .with_deadline(100)
        .with_max_attempts(1);
    let measure_id = measure.id.clone();
    let reconcile =
        AdmissionObligation::new(ObligationKind::Reconcile, "r", "pay", "对账").with_deadline(200);
    let mut guard = ObligationGuard::new();
    guard.record_attempt(&measure);

    let batch = schedule_obligations_by_score_with_guard(
        vec![measure, reconcile],
        50,
        &ObligationBudget {
            tokens: 10_000,
            human: 0,
        },
        &rates,
        &ScorePolicy::default(),
        &guard,
    );
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.scheduled[0].kind, ObligationKind::Reconcile);
    assert!(batch.skipped.iter().any(|skip| {
        skip.id == measure_id && skip.reason == ObligationSkipReason::Thrashed { attempts: 1 }
    }));
}
