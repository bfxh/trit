//! 审计链：追加、验链、重放、篡改与乱序。

use trit_wspace::{AuditLog, Edit, Layer, Net, Patch, Trit, checksum};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn break_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
}

fn fix_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Neg,
    }])
}

fn two_entry_log() -> (AuditLog, Net, Net) {
    let start = xor_net();
    let mut broken = start.clone();
    let break_it = break_patch();
    break_it.apply(&mut broken).expect("break");
    let mut fixed = broken.clone();
    let fix = fix_patch();
    fix.apply(&mut fixed).expect("fix");

    let mut log = AuditLog::new();
    log.record("break", &start, &broken, break_it, true, false)
        .expect("record break");
    log.record("fix", &broken, &fixed, fix, true, false)
        .expect("record fix");
    (log, broken, fixed)
}

#[test]
fn append_verify_replay_round_trip() {
    let (log, broken, fixed) = two_entry_log();
    assert_eq!(log.len(), 2);
    assert!(!log.is_empty());
    log.verify_chain().expect("chain");
    assert_eq!(log.head(), log.entries()[1].chain);
    assert_eq!(log.entries()[0].prev, trit_wspace::audit::GENESIS);

    let (replay, final_net) = log.replay_net(&xor_net()).expect("replay");
    assert_eq!(replay.entries, 2);
    assert_eq!(replay.applied, 2);
    assert_eq!(replay.skipped, 0);
    assert_eq!(replay.rolled_back, 0);
    assert_eq!(replay.final_checksum, checksum(&fixed));
    assert_eq!(final_net, fixed);
    assert_eq!(
        log.replay(&xor_net()).expect("report").final_checksum,
        checksum(&fixed)
    );
    assert_eq!(checksum(&broken), log.entries()[0].after);

    let text = log.to_jsonl();
    let parsed = AuditLog::from_jsonl(&text).expect("parse");
    assert_eq!(parsed, log);
    parsed.verify_chain().expect("parsed chain");
    assert_eq!(parsed.to_jsonl(), text);
}

#[test]
fn tampering_is_detected() {
    let (log, _, _) = two_entry_log();
    let text = log.to_jsonl();
    let tampered = text.replacen("\"to\":\"0\"", "\"to\":\"+\"", 1);
    assert_ne!(tampered, text);
    let parsed = AuditLog::from_jsonl(&tampered).expect("parse tampered");
    assert!(parsed.verify_chain().is_err());
}

#[test]
fn reordering_is_detected() {
    let (log, _, _) = two_entry_log();
    let text = log.to_jsonl();
    let mut lines: Vec<&str> = text.lines().collect();
    lines.swap(0, 1);
    let reordered = format!("{}\n", lines.join("\n"));
    let parsed = AuditLog::from_jsonl(&reordered).expect("parse reordered");
    assert!(parsed.verify_chain().is_err());
}

#[test]
fn rolled_back_and_blocked_entries_replay_unchanged() {
    let start = xor_net();
    let mut after = start.clone();
    let patch = break_patch();
    let journal = patch.apply(&mut after).expect("apply");
    journal.undo(&mut after);
    assert_eq!(after, start);

    let mut log = AuditLog::new();
    log.record("rolled", &start, &after, patch, true, true)
        .expect("record rolled");
    let blocked = fix_patch();
    log.record("blocked", &start, &start.clone(), blocked, false, false)
        .expect("record blocked");

    let (replay, net) = log.replay_net(&start).expect("replay");
    assert_eq!(replay.applied, 0);
    assert_eq!(replay.rolled_back, 1);
    assert_eq!(replay.skipped, 1);
    assert_eq!(net, start);
}

#[test]
fn inconsistent_rollback_and_blocked_entries_are_rejected() {
    let start = xor_net();
    let mut changed = start.clone();
    break_patch().apply(&mut changed).expect("break");
    let mut log = AuditLog::new();
    assert!(
        log.record("bad-rollback", &start, &changed, break_patch(), true, true)
            .is_err()
    );
    assert!(
        log.record("bad-blocked", &start, &changed, break_patch(), false, false)
            .is_err()
    );
    assert!(log.is_empty());
}

#[test]
fn replay_rejects_a_different_initial_artifact() {
    let (log, _, _) = two_entry_log();
    let other = Net::random(&[2, 2, 1], 5).expect("other");
    assert!(log.replay(&other).is_err());
}

#[test]
fn malformed_jsonl_is_rejected() {
    for text in [
        "not json\n",
        "{\"v\":2,\"seq\":0}\n",
        "{\"v\":1,\"seq\":0}\n",
        "{\"v\":1,\"seq\":0,\"before\":\"zz\",\"after\":\"00\",\"prev\":\"00\",\"chain\":\"00\",\"patch\":{\"version\":1,\"edits\":[]}}\n",
    ] {
        assert!(AuditLog::from_jsonl(text).is_err(), "应当拒绝：{text}");
    }
}
