use std::path::PathBuf;

use trit_orch::{
    HandoffPacket, HandoffRecord, RedactionPolicy, Ternary, TraceEvent, TraceEventKind,
    TraceEventStore, TraceLog, TracePayload,
};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-trace-redaction-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn handoff_payload(goal: &str) -> TracePayload {
    TracePayload::Handoff {
        record: Box::new(HandoffRecord {
            packet: HandoffPacket {
                id: "h1".into(),
                from: "alice".into(),
                to: "bob".into(),
                goal: goal.into(),
                base_commit: "abc".into(),
                state_hash: "sha256:abc".into(),
                decisions: Vec::new(),
                open_questions: Vec::new(),
                claims: Vec::new(),
                evidence: Vec::new(),
                next_action: "run gates".into(),
                idempotency_key: "handoff:h1".into(),
                expires_at: 200,
            },
            receiver_state_hash: "sha256:abc".into(),
            gate: Ternary::True,
            obligation: None,
        }),
    }
}

#[test]
fn redacted_event_keeps_identity_and_commits_payload_hash() {
    let first = TraceEvent::new("t1", 1, TraceEventKind::HandoffGated, "h1", 100)
        .with_id("e1")
        .with_payload(handoff_payload("Bearer sk-abc123"));
    let second = TraceEvent::new("t1", 2, TraceEventKind::ObligationScheduled, "h1", 101)
        .with_id("e2")
        .with_causation_id("e1");

    let redacted = first.redacted();
    assert_eq!(redacted.id, "e1");
    assert_eq!(redacted.seq, 1);
    assert!(redacted.payload.is_none());
    assert!(
        redacted.payload_hash.starts_with("fnv1a64:"),
        "脱敏后仍要保留原 payload 的承诺"
    );
    assert_eq!(
        redacted.payload_hash,
        first.redacted().payload_hash,
        "同一 payload 的哈希必须确定"
    );

    let other = TraceEvent::new("t1", 1, TraceEventKind::HandoffGated, "h1", 100)
        .with_id("e1")
        .with_payload(handoff_payload("different goal"));
    assert_ne!(
        redacted.payload_hash,
        other.redacted().payload_hash,
        "内容不同，哈希必须不同"
    );

    let mut log = TraceLog::new("t1");
    log.append(redacted).unwrap();
    log.append(second).unwrap();
    let chain = log.why("e2").unwrap();
    assert_eq!(
        chain
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["e1", "e2"],
        "payload 脱敏不能破坏 seq / 因果审计"
    );
}

#[test]
fn redaction_policy_scrubs_secrets_without_dropping_payload_shape() {
    let event = TraceEvent::new("t1", 1, TraceEventKind::HandoffGated, "h1", 100)
        .with_payload(handoff_payload("use Bearer sk-abc123 to finish"));

    let redacted = event.redacted_with(&RedactionPolicy::default());
    let Some(TracePayload::Handoff { record }) = redacted.payload else {
        panic!("结构没变时应该保留 payload");
    };
    assert_eq!(record.packet.goal, "[REDACTED]");
    assert_eq!(record.packet.id, "h1", "非敏感字段必须保留");
    assert!(redacted.payload_hash.starts_with("fnv1a64:"));
}

#[test]
fn store_redacts_then_compacts_old_payloads_without_dropping_events() {
    let path = temp_path("retention");
    let store = TraceEventStore::new(&path);
    let old = TraceEvent::new("t1", 1, TraceEventKind::HandoffGated, "h1", 100)
        .with_payload(handoff_payload("use Bearer sk-old"));
    let new = TraceEvent::new("t1", 2, TraceEventKind::HandoffGated, "h2", 200)
        .with_payload(handoff_payload("use Bearer sk-new"))
        .with_causation_id("t1:1");
    store.append(&old).unwrap();
    store.append(&new).unwrap();

    let count = store
        .redact_before(150, &RedactionPolicy::default())
        .unwrap();
    assert_eq!(count, 1);
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 2, "判定事件长期保留");
    let Some(TracePayload::Handoff { record }) = loaded[0].payload.as_ref() else {
        panic!("旧 payload 应该存在但已脱敏");
    };
    assert_eq!(record.packet.goal, "[REDACTED]");
    let Some(TracePayload::Handoff { record }) = loaded[1].payload.as_ref() else {
        panic!("新 payload 应该保留");
    };
    assert_eq!(record.packet.goal, "use Bearer sk-new");

    let count = store.compact_before(150).unwrap();
    assert_eq!(count, 1);
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 2, "压缩只删载荷，不删事件");
    assert!(loaded[0].payload.is_none());
    assert!(loaded[0].payload_hash.starts_with("fnv1a64:"));
    assert!(loaded[1].payload.is_some());

    let log = store.load_log("t1").unwrap();
    let chain = log.why("t1:2").unwrap();
    assert_eq!(chain.events.len(), 2, "压缩后因果链仍可审计");
}
