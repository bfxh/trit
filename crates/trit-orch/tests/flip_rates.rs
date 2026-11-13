use trit_orch::{
    AdmissionObligation, BudgetBook, BudgetSplit, EvidenceVerdict, FlipRates, ObligationBudget,
    ObligationKind, ObligationOutcome, ScorePolicy, TraceEvent, TraceEventKind, TraceLog,
    TracePayload, schedule_obligations_by_score, schedule_obligations_with_budget_by_score,
};

fn judgment(seq: u64, subject: &str, verdict: EvidenceVerdict) -> TraceEvent {
    TraceEvent::new("t1", seq, TraceEventKind::JudgmentDerived, subject, 100).with_payload(
        TracePayload::Judgment {
            subject: subject.into(),
            verdict,
            obligations: Vec::new(),
        },
    )
}

fn fulfilled(seq: u64, task: &str, kind: ObligationKind) -> TraceEvent {
    let obligation = AdmissionObligation::new(kind, task, "dep", "test");
    TraceEvent::new("t1", seq, TraceEventKind::ObligationFulfilled, task, 100).with_payload(
        TracePayload::Obligation {
            outcome: ObligationOutcome::Fulfilled,
            obligation,
        },
    )
}

#[test]
fn flip_rates_use_laplace_smoothing_and_counts() {
    let mut rates = FlipRates::default();
    assert_eq!(rates.probability_bp(ObligationKind::Measure), 5_000);

    rates.record(ObligationKind::Measure, true);
    rates.record(ObligationKind::Measure, true);
    rates.record(ObligationKind::Measure, false);
    assert_eq!(rates.resolved(ObligationKind::Measure), 3);
    assert_eq!(rates.flipped(ObligationKind::Measure), 2);
    assert_eq!(rates.probability_bp(ObligationKind::Measure), 6_000);
}

#[test]
fn flip_rates_learn_from_trace_only_unknown_to_decided() {
    let mut log = TraceLog::new("t1");
    for event in [
        judgment(1, "a", EvidenceVerdict::Neither),
        fulfilled(2, "a", ObligationKind::Measure),
        judgment(3, "a", EvidenceVerdict::Proved),
        judgment(4, "b", EvidenceVerdict::Neither),
        fulfilled(5, "b", ObligationKind::Measure),
        judgment(6, "b", EvidenceVerdict::Neither),
        judgment(7, "c", EvidenceVerdict::Proved),
        fulfilled(8, "c", ObligationKind::Measure),
        judgment(9, "c", EvidenceVerdict::Proved),
        judgment(10, "d", EvidenceVerdict::Neither),
        fulfilled(11, "d", ObligationKind::Measure),
    ] {
        log.append(event).unwrap();
    }

    let rates = FlipRates::from_trace(&log);
    assert_eq!(rates.resolved(ObligationKind::Measure), 2);
    assert_eq!(rates.flipped(ObligationKind::Measure), 1);
}

#[test]
fn score_orders_by_expected_gain_per_cost_not_edf() {
    let mut rates = FlipRates::default();
    for _ in 0..3 {
        rates.record(ObligationKind::Measure, true);
    }
    for _ in 0..3 {
        rates.record(ObligationKind::Reconcile, false);
    }

    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "m", "dep", "补证").with_deadline(200);
    let reconcile =
        AdmissionObligation::new(ObligationKind::Reconcile, "r", "pay", "对账").with_deadline(100);
    let budget = ObligationBudget {
        tokens: 10_000,
        human: 0,
    };

    let batch = schedule_obligations_by_score(
        vec![reconcile, measure],
        50,
        &budget,
        &rates,
        &ScorePolicy::default(),
    );
    assert_eq!(batch.scheduled[0].kind, ObligationKind::Measure);
    assert_eq!(batch.scheduled[1].kind, ObligationKind::Reconcile);
}

#[test]
fn score_scheduler_skips_over_budget_without_head_of_line_blocking() {
    let mut rates = FlipRates::default();
    for _ in 0..3 {
        rates.record(ObligationKind::Measure, true);
    }
    for _ in 0..3 {
        rates.record(ObligationKind::Reconcile, false);
    }

    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "m", "dep", "补证").with_deadline(100);
    let reconcile =
        AdmissionObligation::new(ObligationKind::Reconcile, "r", "pay", "对账").with_deadline(200);
    let budget = ObligationBudget {
        tokens: 300,
        human: 0,
    };

    let batch = schedule_obligations_by_score(
        vec![measure, reconcile],
        50,
        &budget,
        &rates,
        &ScorePolicy::default(),
    );
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.scheduled[0].kind, ObligationKind::Reconcile);
    assert_eq!(batch.tokens_used, 200);
}

#[test]
fn budgeted_score_scheduler_uses_same_order_and_pools() {
    let mut rates = FlipRates::default();
    for _ in 0..3 {
        rates.record(ObligationKind::Measure, true);
    }
    for _ in 0..3 {
        rates.record(ObligationKind::Reconcile, false);
    }

    let measure =
        AdmissionObligation::new(ObligationKind::Measure, "m", "dep", "补证").with_deadline(200);
    let reconcile =
        AdmissionObligation::new(ObligationKind::Reconcile, "r", "pay", "对账").with_deadline(100);
    let mut book = BudgetBook::new(BudgetSplit {
        execution: 0,
        verification: 10_000,
        reconciliation: 10_000,
        human: 0,
    });

    let batch = schedule_obligations_with_budget_by_score(
        vec![reconcile, measure],
        50,
        &mut book,
        &rates,
        &ScorePolicy::default(),
    );
    assert_eq!(batch.scheduled[0].kind, ObligationKind::Measure);
    assert_eq!(batch.scheduled[1].kind, ObligationKind::Reconcile);
    assert_eq!(batch.budget.verification_spent, 500);
    assert_eq!(batch.budget.reconciliation_spent, 200);
}
