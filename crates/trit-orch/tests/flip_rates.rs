use trit_orch::{
    AdmissionObligation, BudgetBook, BudgetSplit, Error, EvidenceVerdict, FlipPrior, FlipRates,
    ObligationBudget, ObligationKind, ObligationOutcome, ScorePolicy, TraceEvent, TraceEventKind,
    TraceLog, TracePayload, schedule_obligations_by_score,
    schedule_obligations_with_budget_by_score,
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

#[test]
fn cold_start_uses_per_kind_priors() {
    let mut rates = FlipRates::default().with_prior(ObligationKind::Measure, FlipPrior::new(3, 1));
    assert_eq!(
        rates.probability_bp(ObligationKind::Measure),
        7_500,
        "ORCH_FLIP_PRIOR"
    );
    assert_eq!(rates.probability_bp(ObligationKind::Reconcile), 5_000);

    rates.record(ObligationKind::Measure, false);
    assert_eq!(
        rates.probability_bp(ObligationKind::Measure),
        6_000,
        "先验只做伪计数，观测到了就让位"
    );
}

#[test]
fn empirical_calibration_sets_prior_without_counting_corpus() {
    let mut log = TraceLog::new("t1");
    for event in [
        judgment(1, "a", EvidenceVerdict::Neither),
        fulfilled(2, "a", ObligationKind::Measure),
        judgment(3, "a", EvidenceVerdict::Proved),
        judgment(4, "b", EvidenceVerdict::Neither),
        fulfilled(5, "b", ObligationKind::Measure),
        judgment(6, "b", EvidenceVerdict::Neither),
        judgment(7, "c", EvidenceVerdict::Neither),
        fulfilled(8, "c", ObligationKind::Measure),
        judgment(9, "c", EvidenceVerdict::Proved),
    ] {
        log.append(event).unwrap();
    }

    let rates = FlipRates::calibrated_from_trace(&log, 100);
    assert_eq!(
        rates.resolved(ObligationKind::Measure),
        0,
        "ORCH_FLIP_CALIBRATION_COUNTS: 校准语料只估先验，不再计数"
    );
    let prior = rates.prior(ObligationKind::Measure);
    assert_eq!(
        (prior.alpha(), prior.beta()),
        (67, 33),
        "ORCH_FLIP_CALIBRATION_PRIOR"
    );
    assert_eq!(prior.mean_bp(), 6_700);
    assert_eq!(
        rates.probability_bp(ObligationKind::Measure),
        6_700,
        "ORCH_FLIP_CALIBRATION_MEAN"
    );
    assert_eq!(
        rates.probability_bp(ObligationKind::Reconcile),
        5_000,
        "未见过的 kind 保持均匀先验"
    );
}

#[test]
fn zero_strength_calibration_and_zero_prior_fall_back_to_uniform() {
    let log = TraceLog::new("t1");
    let rates = FlipRates::calibrated_from_trace(&log, 0);
    assert_eq!(rates.probability_bp(ObligationKind::Measure), 5_000);

    let mut zero = FlipRates::default().with_prior(ObligationKind::Measure, FlipPrior::new(0, 0));
    assert_eq!(
        zero.probability_bp(ObligationKind::Measure),
        5_000,
        "ORCH_FLIP_ZERO_PRIOR"
    );
    zero.record(ObligationKind::Measure, true);
    assert_eq!(zero.probability_bp(ObligationKind::Measure), 10_000);
    zero.record(ObligationKind::Measure, false);
    assert_eq!(zero.probability_bp(ObligationKind::Measure), 5_000);
}

#[test]
fn windowed_flip_rates_keep_only_recent_samples() {
    let mut rates = FlipRates::windowed(2).unwrap();
    rates.record(ObligationKind::Measure, true);
    rates.record(ObligationKind::Measure, true);
    rates.record(ObligationKind::Measure, false);

    assert_eq!(rates.window(), Some(2));
    assert_eq!(
        rates.resolved(ObligationKind::Measure),
        2,
        "ORCH_FLIP_WINDOW"
    );
    assert_eq!(rates.flipped(ObligationKind::Measure), 1);
    assert_eq!(
        rates.probability_bp(ObligationKind::Measure),
        5_000,
        "旧样本滑出窗口后不再参与平滑"
    );
}

#[test]
fn windowed_flip_rates_follow_drift() {
    let mut rates = FlipRates::windowed(3).unwrap();
    for _ in 0..3 {
        rates.record(ObligationKind::Measure, false);
    }
    assert_eq!(
        rates.probability_bp(ObligationKind::Measure),
        2_000,
        "默认 Laplace 先验下的全否样本"
    );

    for _ in 0..3 {
        rates.record(ObligationKind::Measure, true);
    }
    assert_eq!(rates.resolved(ObligationKind::Measure), 3);
    assert_eq!(rates.flipped(ObligationKind::Measure), 3);
    assert_eq!(
        rates.probability_bp(ObligationKind::Measure),
        8_000,
        "ORCH_FLIP_DRIFT"
    );
}

#[test]
fn prior_share_is_bounded_by_the_window() {
    let mut rates = FlipRates::windowed(8)
        .unwrap()
        .with_prior(ObligationKind::Measure, FlipPrior::new(1, 1));
    rates.record(ObligationKind::Measure, true);
    assert_eq!(rates.prior_share_bp(ObligationKind::Measure), 6_666);

    for _ in 0..7 {
        rates.record(ObligationKind::Measure, false);
    }
    assert_eq!(rates.prior_share_bp(ObligationKind::Measure), 2_000);

    for _ in 0..20 {
        rates.record(ObligationKind::Measure, false);
    }
    assert_eq!(
        rates.prior_share_bp(ObligationKind::Measure),
        2_000,
        "ORCH_FLIP_PRIOR_SHARE_STABLE: 窗口填满后先验占比不再漂移"
    );
}

#[test]
fn windowed_ingestion_replays_only_the_tail() {
    let mut log = TraceLog::new("t1");
    for event in [
        judgment(1, "a", EvidenceVerdict::Neither),
        fulfilled(2, "a", ObligationKind::Measure),
        judgment(3, "a", EvidenceVerdict::Proved),
        judgment(4, "b", EvidenceVerdict::Neither),
        fulfilled(5, "b", ObligationKind::Measure),
        judgment(6, "b", EvidenceVerdict::Proved),
        judgment(7, "c", EvidenceVerdict::Neither),
        fulfilled(8, "c", ObligationKind::Measure),
        judgment(9, "c", EvidenceVerdict::Neither),
    ] {
        log.append(event).unwrap();
    }

    let rates = FlipRates::windowed_from_trace(&log, 2).unwrap();
    assert_eq!(rates.resolved(ObligationKind::Measure), 2);
    assert_eq!(
        rates.flipped(ObligationKind::Measure),
        1,
        "ORCH_FLIP_WINDOW_TRACE"
    );
}

#[test]
fn zero_window_is_rejected() {
    assert_eq!(
        FlipRates::windowed(0),
        Err(Error::FlipWindowZero),
        "ORCH_FLIP_WINDOW_ZERO"
    );
}
