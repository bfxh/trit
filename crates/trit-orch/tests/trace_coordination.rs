use std::path::PathBuf;

use trit_orch::{
    AdmissionObligation, Claim, ClaimAction, ClaimOutcome, ClaimRecord, ClaimStore, ContextFacts,
    DeliveryReceipt, Effect, EffectRecord, HandoffPacket, HandoffRecord, Ledger, ObligationKind,
    ObligationOutcome, ObligationRecord, ReceiptAction, ReceiptRecord, ReceiptState, Ternary,
    TraceEventKind, TraceEventStore, TraceLog, TraceRecorder, compaction_gate, handoff_gate,
    handoff_obligation,
};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-trace-coordination-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn claim() -> Claim {
    match ClaimStore::new()
        .claim("alice", &["src/a.rs".into()], "edit", 100, 10)
        .unwrap()
    {
        ClaimOutcome::Acquired(claim) => claim,
        other => panic!("期望 Acquired，得到 {other:?}"),
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

fn facts() -> ContextFacts {
    ContextFacts {
        measured_tokens: Some(60_000),
        context_window: 100_000,
        threshold_bp: 5_100,
        ..ContextFacts::default()
    }
}

#[test]
fn recorder_emits_claim_and_receipt_events() {
    let claim = claim();
    let sent = DeliveryReceipt::sent("r1", "k1").unwrap();
    let mut read = sent.clone();
    read.advance(ReceiptState::Delivered).unwrap();
    read.advance(ReceiptState::Read).unwrap();

    let mut recorder = TraceRecorder::new("t1", 1);
    let claim_event = recorder.claim(ClaimAction::Acquired, claim.clone(), 100);
    let sent_event = recorder.receipt(ReceiptAction::Recorded, sent.clone(), 101);
    let read_event = recorder.receipt(ReceiptAction::Advanced, read.clone(), 102);

    assert_eq!(claim_event.kind, TraceEventKind::ClaimAcquired);
    assert_eq!(claim_event.correlation_id, claim.id);
    assert_eq!(claim_event.seq, 1);
    assert_eq!(sent_event.kind, TraceEventKind::ReceiptRecorded);
    assert_eq!(sent_event.correlation_id, "k1");
    assert_eq!(read_event.kind, TraceEventKind::ReceiptAdvanced);
    assert_eq!(read_event.seq, 3);

    let mut log = TraceLog::new("t1");
    for event in [claim_event, sent_event, read_event] {
        log.append(event).unwrap();
    }
    let fold = log.coordination();
    assert_eq!(
        fold.claims,
        vec![ClaimRecord {
            action: ClaimAction::Acquired,
            claim,
        }]
    );
    assert_eq!(
        fold.receipts,
        vec![
            ReceiptRecord {
                action: ReceiptAction::Recorded,
                receipt: sent,
            },
            ReceiptRecord {
                action: ReceiptAction::Advanced,
                receipt: read,
            }
        ]
    );
}

#[test]
fn recorder_emits_handoff_compaction_and_obligation_events() {
    let packet = packet();
    let gate = handoff_gate(&packet, "sha256:other", 100);
    let obligation = handoff_obligation(&packet, "sha256:other", 100);
    assert_eq!(gate, Ternary::Unknown);
    assert!(obligation.is_some(), "哈希不一致必须产出义务");

    let handoff = HandoffRecord {
        packet: packet.clone(),
        receiver_state_hash: "sha256:other".into(),
        gate,
        obligation: obligation.clone(),
    };
    let decision = compaction_gate(&facts()).unwrap();
    let admission = AdmissionObligation::new(ObligationKind::Measure, "a", "dep", "补证");

    let mut recorder = TraceRecorder::new("t1", 1);
    let handoff_event = recorder.handoff(handoff.clone(), 100);
    let compaction_event = recorder.compaction(decision.clone(), 101);
    let obligation_event =
        recorder.obligation(ObligationOutcome::Scheduled, admission.clone(), 102);

    assert_eq!(handoff_event.kind, TraceEventKind::HandoffGated);
    assert_eq!(handoff_event.correlation_id, packet.id);
    assert_eq!(compaction_event.kind, TraceEventKind::CompactionDecided);
    assert_eq!(obligation_event.kind, TraceEventKind::ObligationScheduled);

    let mut log = TraceLog::new("t1");
    for event in [handoff_event, compaction_event, obligation_event] {
        log.append(event).unwrap();
    }
    let fold = log.coordination();
    assert_eq!(fold.handoffs, vec![handoff]);
    assert_eq!(fold.compactions, vec![decision]);
    assert_eq!(
        fold.obligations,
        vec![ObligationRecord {
            outcome: ObligationOutcome::Scheduled,
            obligation: admission,
        }]
    );
}

#[test]
fn recorder_emits_effect_transitions() {
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("pay", 0, "pay", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();

    let mut recorder = TraceRecorder::new("t1", 1);
    let event = recorder.effect(attempt.clone(), Effect::Sending, 100);
    assert_eq!(event.kind, TraceEventKind::EffectSending);
    assert_eq!(event.correlation_id, "pay");

    let mut log = TraceLog::new("t1");
    log.append(event).unwrap();
    assert_eq!(
        log.coordination().effects,
        vec![EffectRecord {
            attempt,
            state: Effect::Sending,
        }]
    );
}

#[test]
fn coordination_events_roundtrip_through_event_store() {
    let path = temp_path("roundtrip");
    let store = TraceEventStore::new(&path);
    let claim = claim();
    let admission = AdmissionObligation::new(ObligationKind::Measure, "a", "dep", "补证");
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("pay", 0, "pay", 1).unwrap();
    let mut recorder = TraceRecorder::new("t1", 1);
    let first = recorder.claim(ClaimAction::Renewed, claim, 100);
    let second = recorder.obligation(ObligationOutcome::Fulfilled, admission, 101);
    let third = recorder.effect(attempt, Effect::Prepared, 102);
    store.append(&first).unwrap();
    store.append(&second).unwrap();
    store.append(&third).unwrap();

    let log = store.load_log("t1").unwrap();
    let fold = log.coordination();
    assert_eq!(fold.claims.len(), 1);
    assert_eq!(fold.claims[0].action, ClaimAction::Renewed);
    assert_eq!(fold.obligations.len(), 1);
    assert_eq!(fold.obligations[0].outcome, ObligationOutcome::Fulfilled);
    assert_eq!(fold.effects.len(), 1);
    assert_eq!(fold.effects[0].state, Effect::Prepared);
}
