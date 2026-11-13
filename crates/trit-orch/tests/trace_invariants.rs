use trit_orch::{
    AdmissionObligation, Attempt, ClaimAction, ClaimOutcome, ClaimStore, DeliveryReceipt, Effect,
    EvidenceVerdict, FinishCounts, HandoffPacket, HandoffRecord, ObligationKind, ReceiptAction,
    ReceiptState, Reconcile, Ternary, TraceEvent, TraceEventKind, TraceInvariantViolation,
    TraceLog, TracePayload, TraceRecorder,
};

fn attempt() -> Attempt {
    Attempt {
        operation: "pay".into(),
        task: "checkout".into(),
        epoch: 0,
        attempt: 1,
        fence: 1,
    }
}

fn packet() -> HandoffPacket {
    HandoffPacket {
        id: "h1".into(),
        from: "alice".into(),
        to: "bob".into(),
        goal: "finish".into(),
        base_commit: "abc".into(),
        state_hash: "sha256:abc".into(),
        decisions: Vec::new(),
        open_questions: Vec::new(),
        claims: Vec::new(),
        evidence: Vec::new(),
        next_action: "run gates".into(),
        idempotency_key: "handoff:h1".into(),
        expires_at: 200,
    }
}

fn claim() -> trit_orch::Claim {
    match ClaimStore::new()
        .claim("alice", &["src/a.rs".into()], "edit", 100, 10)
        .unwrap()
    {
        ClaimOutcome::Acquired(claim) => claim,
        other => panic!("期望 Acquired，得到 {other:?}"),
    }
}

fn clean_events() -> Vec<TraceEvent> {
    let mut recorder = TraceRecorder::new("t1", 1);
    let attempt = attempt();
    let sent = DeliveryReceipt::sent("r1", "k1").unwrap();
    let mut delivered = sent.clone();
    delivered.advance(ReceiptState::Delivered).unwrap();
    let mut read = delivered.clone();
    read.advance(ReceiptState::Read).unwrap();
    let mut acted = read.clone();
    acted.advance(ReceiptState::Acted).unwrap();
    let handoff = HandoffRecord {
        packet: packet(),
        receiver_state_hash: "sha256:abc".into(),
        gate: Ternary::True,
        obligation: None,
    };

    vec![
        recorder.claim(ClaimAction::Acquired, claim(), 100),
        recorder.effect(attempt.clone(), Effect::Prepared, 100),
        recorder.effect(attempt.clone(), Effect::Sending, 100),
        recorder.effect(attempt.clone(), Effect::Sent, 100),
        recorder.effect(attempt, Effect::Confirmed, 100),
        recorder.receipt(ReceiptAction::Recorded, sent, 100),
        recorder.receipt(ReceiptAction::Advanced, delivered, 100),
        recorder.receipt(ReceiptAction::Advanced, read, 100),
        recorder.receipt(ReceiptAction::Advanced, acted, 100),
        recorder.handoff(handoff, 100),
        recorder.judgment("j1", EvidenceVerdict::Proved, Vec::new(), 100),
        recorder.finish(
            true,
            FinishCounts {
                confirmed: 1,
                refuted: 0,
                undetermined: 0,
                conflicts: 0,
                in_doubt: 0,
            },
            100,
        ),
    ]
}

fn log_from(events: Vec<TraceEvent>) -> TraceLog {
    let mut log = TraceLog::new("t1");
    for event in events {
        log.append(event).unwrap();
    }
    log
}

fn unknown_effects(recorder: &mut TraceRecorder, attempt: &Attempt) -> Vec<TraceEvent> {
    vec![
        recorder.effect(attempt.clone(), Effect::Prepared, 100),
        recorder.effect(attempt.clone(), Effect::Sending, 100),
        recorder.effect(attempt.clone(), Effect::Unknown, 100),
    ]
}

#[test]
fn clean_trace_passes_all_invariants() {
    let report = log_from(clean_events()).check_invariants();
    assert!(report.is_clean(), "{report:#?}");
}

#[test]
fn deleting_an_event_is_caught() {
    let mut events = clean_events();
    events.retain(|event| {
        !matches!(&event.payload, Some(TracePayload::Effect { state, .. }) if *state == Effect::Sending)
    });
    let report = log_from(events).check_invariants();
    assert!(!report.is_clean());
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, TraceInvariantViolation::MissingSeq { .. }))
    );
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, TraceInvariantViolation::EffectOutOfOrder { .. }))
    );
}

#[test]
fn reordering_events_is_caught() {
    let mut events = clean_events();
    let mut sending_seq = None;
    let mut sent_seq = None;
    for event in &events {
        if let Some(TracePayload::Effect { state, .. }) = &event.payload {
            match state {
                Effect::Sending => sending_seq = Some(event.seq),
                Effect::Sent => sent_seq = Some(event.seq),
                _ => {}
            }
        }
    }
    let sending_seq = sending_seq.unwrap();
    let sent_seq = sent_seq.unwrap();
    for event in &mut events {
        if event.seq == sending_seq {
            event.seq = sent_seq;
        } else if event.seq == sent_seq {
            event.seq = sending_seq;
        }
    }

    let report = log_from(events).check_invariants();
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, TraceInvariantViolation::EffectOutOfOrder { .. }))
    );
}

#[test]
fn lowering_a_fence_is_caught() {
    let mut events = clean_events();
    for event in &mut events {
        if let Some(TracePayload::Effect { attempt, state }) = &mut event.payload
            && *state == Effect::Sending
        {
            attempt.fence = 0;
        }
    }

    let report = log_from(events).check_invariants();
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, TraceInvariantViolation::StaleFence { .. }))
    );
}

#[test]
fn acted_without_read_is_caught() {
    let mut recorder = TraceRecorder::new("t1", 1);
    let sent = DeliveryReceipt::sent("r1", "k1").unwrap();
    let mut acted = sent.clone();
    acted.advance(ReceiptState::Delivered).unwrap();
    acted.advance(ReceiptState::Read).unwrap();
    acted.advance(ReceiptState::Acted).unwrap();
    let log = log_from(vec![
        recorder.receipt(ReceiptAction::Recorded, sent, 100),
        recorder.receipt(ReceiptAction::Advanced, acted, 100),
    ]);

    let report = log.check_invariants();
    assert!(
        report.violations.iter().any(|violation| matches!(
            violation,
            TraceInvariantViolation::ReceiptOutOfOrder { .. }
        ))
    );
}

#[test]
fn handoff_without_obligation_is_caught() {
    let event = TraceRecorder::new("t1", 1).handoff(
        HandoffRecord {
            packet: packet(),
            receiver_state_hash: "sha256:other".into(),
            gate: Ternary::Unknown,
            obligation: None,
        },
        100,
    );

    let report = log_from(vec![event]).check_invariants();
    assert!(report.violations.iter().any(|violation| matches!(
        violation,
        TraceInvariantViolation::HandoffWithoutObligation { .. }
    )));
}

#[test]
fn missing_causation_is_caught() {
    let event = TraceEvent::new("t1", 1, TraceEventKind::TaskSelected, "c", 100)
        .with_id("e1")
        .with_causation_id("ghost");
    let report = log_from(vec![event]).check_invariants();
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, TraceInvariantViolation::MissingCausation { .. }))
    );
}

#[test]
fn unknown_judgment_without_obligation_is_caught() {
    let mut recorder = TraceRecorder::new("t1", 1);
    let bad = recorder.judgment("j1", EvidenceVerdict::Neither, Vec::new(), 100);
    let report = log_from(vec![bad]).check_invariants();
    assert!(report.violations.iter().any(|violation| matches!(
        violation,
        TraceInvariantViolation::UnknownWithoutObligation { .. }
    )));

    let measure = AdmissionObligation::new(ObligationKind::Measure, "j1", "dep", "补证");
    let mut recorder = TraceRecorder::new("t1", 1);
    let good = recorder.judgment("j1", EvidenceVerdict::Neither, vec![measure], 100);
    assert!(log_from(vec![good]).check_invariants().is_clean());
}

#[test]
fn conflicted_judgment_without_adjudication_is_caught() {
    let measure = AdmissionObligation::new(ObligationKind::Measure, "j1", "dep", "补证");
    let adjudicate = AdmissionObligation::new(ObligationKind::Adjudicate, "j1", "dep", "裁决");

    let mut recorder = TraceRecorder::new("t1", 1);
    let bad = recorder.judgment("j1", EvidenceVerdict::Conflicted, vec![measure], 100);
    let report = log_from(vec![bad]).check_invariants();
    assert!(report.violations.iter().any(|violation| matches!(
        violation,
        TraceInvariantViolation::ConflictWithoutAdjudication { .. }
    )));

    let mut recorder = TraceRecorder::new("t1", 1);
    let good = recorder.judgment("j1", EvidenceVerdict::Conflicted, vec![adjudicate], 100);
    assert!(log_from(vec![good]).check_invariants().is_clean());
}

#[test]
fn confirmed_finish_with_undetermined_is_caught() {
    let mut recorder = TraceRecorder::new("t1", 1);
    let bad = recorder.finish(
        true,
        FinishCounts {
            confirmed: 1,
            refuted: 0,
            undetermined: 1,
            conflicts: 0,
            in_doubt: 0,
        },
        100,
    );
    let report = log_from(vec![bad]).check_invariants();
    assert!(report.violations.iter().any(|violation| matches!(
        violation,
        TraceInvariantViolation::FinishWithUndetermined { .. }
    )));

    let mut recorder = TraceRecorder::new("t1", 1);
    let good = recorder.finish(
        true,
        FinishCounts {
            confirmed: 1,
            refuted: 0,
            undetermined: 0,
            conflicts: 0,
            in_doubt: 0,
        },
        100,
    );
    assert!(log_from(vec![good]).check_invariants().is_clean());

    let mut recorder = TraceRecorder::new("t1", 1);
    let refused = recorder.finish(
        false,
        FinishCounts {
            confirmed: 0,
            refuted: 0,
            undetermined: 1,
            conflicts: 0,
            in_doubt: 0,
        },
        100,
    );
    assert!(
        log_from(vec![refused]).check_invariants().is_clean(),
        "拒绝报完成是诚实的"
    );
}

#[test]
fn confirmed_finish_without_payload_is_caught() {
    let event = TraceEvent::new("t1", 1, TraceEventKind::FinishConfirmed, "finish", 100);
    let report = log_from(vec![event]).check_invariants();
    assert!(report.violations.iter().any(|violation| matches!(
        violation,
        TraceInvariantViolation::FinishWithoutPayload { .. }
    )));
}

#[test]
fn reconcile_payload_preserves_observed_from_and_to() {
    let attempt = attempt();
    let mut recorder = TraceRecorder::new("t1", 1);
    let event = recorder.effect_reconciled(
        attempt.clone(),
        Reconcile::Present,
        Effect::Unknown,
        Effect::Confirmed,
        100,
    );
    assert_eq!(event.kind, TraceEventKind::EffectReconciled);
    assert_eq!(event.correlation_id, "pay");
    let Some(TracePayload::EffectReconciled {
        attempt: recorded,
        observed,
        from,
        to,
    }) = event.payload
    else {
        panic!("EffectReconciled 必须带载荷");
    };
    assert_eq!(recorded, attempt);
    assert_eq!(observed, Reconcile::Present);
    assert_eq!(from, Effect::Unknown);
    assert_eq!(to, Effect::Confirmed);
}

#[test]
fn confirmed_after_unknown_requires_present_reconcile() {
    let attempt = attempt();
    let mut recorder = TraceRecorder::new("t1", 1);
    let mut missing = unknown_effects(&mut recorder, &attempt);
    missing.push(recorder.effect(attempt.clone(), Effect::Confirmed, 100));
    let report = log_from(missing).check_invariants();
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, TraceInvariantViolation::ReconcileMissing { .. }))
    );

    let mut recorder = TraceRecorder::new("t1", 1);
    let mut reconciled = unknown_effects(&mut recorder, &attempt);
    reconciled.push(recorder.effect_reconciled(
        attempt.clone(),
        Reconcile::Present,
        Effect::Unknown,
        Effect::Confirmed,
        100,
    ));
    reconciled.push(recorder.effect(attempt.clone(), Effect::Confirmed, 100));
    assert!(log_from(reconciled).check_invariants().is_clean());
}

#[test]
fn refuted_after_unknown_requires_absent_reconcile() {
    let attempt = attempt();
    let mut recorder = TraceRecorder::new("t1", 1);
    let mut missing = unknown_effects(&mut recorder, &attempt);
    missing.push(recorder.effect(attempt.clone(), Effect::Refuted, 100));
    let report = log_from(missing).check_invariants();
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, TraceInvariantViolation::ReconcileMissing { .. }))
    );

    let mut recorder = TraceRecorder::new("t1", 1);
    let mut reconciled = unknown_effects(&mut recorder, &attempt);
    reconciled.push(recorder.effect_reconciled(
        attempt.clone(),
        Reconcile::Absent,
        Effect::Unknown,
        Effect::Refuted,
        100,
    ));
    reconciled.push(recorder.effect(attempt.clone(), Effect::Refuted, 100));
    assert!(log_from(reconciled).check_invariants().is_clean());
}

#[test]
fn reconcile_record_must_match_from_and_observed() {
    let attempt = attempt();
    let mut recorder = TraceRecorder::new("t1", 1);
    let mut events = unknown_effects(&mut recorder, &attempt);
    events.push(recorder.effect_reconciled(
        attempt.clone(),
        Reconcile::Present,
        Effect::Sent,
        Effect::Confirmed,
        100,
    ));
    events.push(recorder.effect(attempt.clone(), Effect::Confirmed, 100));

    let report = log_from(events).check_invariants();
    assert!(
        report.violations.iter().any(|violation| matches!(
            violation,
            TraceInvariantViolation::ReconcileMismatch { .. }
        ))
    );
}

#[test]
fn reconcile_without_payload_is_caught() {
    let event = TraceEvent::new("t1", 1, TraceEventKind::EffectReconciled, "pay", 100);
    let report = log_from(vec![event]).check_invariants();
    assert!(report.violations.iter().any(|violation| matches!(
        violation,
        TraceInvariantViolation::ReconcileWithoutPayload { .. }
    )));
}
