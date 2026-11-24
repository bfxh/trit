//! `delivery check` 的只读预检：交接门 + 账本状态，不写账本。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use trit_orch::delivery::{self, DeliveryJournal};

#[allow(dead_code)]
mod cli_support;
use cli_support::run;

fn root(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!(
        "trit-delivery-check-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn packet(id: &str, state_hash: &str) -> Value {
    json!({
        "id": id,
        "from": "agent-a",
        "to": "agent-b",
        "goal": "hand off the ledger",
        "base_commit": "6143af9e",
        "state_hash": state_hash,
        "decisions": ["keep it in rust"],
        "open_questions": [],
        "claims": [],
        "evidence": [],
        "next_action": "read the ledger",
        "idempotency_key": format!("{id}-key"),
        "expires_at": 10_000_000_000u64,
    })
}

fn write_packet(root: &Path, id: &str, state_hash: &str) -> PathBuf {
    let path = root.join("packet.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&packet(id, state_hash)).unwrap(),
    )
    .unwrap();
    path
}

#[test]
fn check_is_read_only_and_reports_gate_plus_missing_ledger() {
    let root = root("readonly");
    let path = write_packet(&root, "c1", "H");

    let report = delivery::check(&root, &path, "not-H", 100).unwrap();
    assert_eq!(report.gate, trit_orch::Ternary::Unknown);
    assert_eq!(report.receipt.status, "missing");
    assert!(!report.ready);
    assert!(!DeliveryJournal::path(&root, "c1").exists());

    let report = delivery::check(&root, &path, "H", 100).unwrap();
    assert_eq!(report.gate, trit_orch::Ternary::True);
    assert_eq!(report.receipt.status, "missing");
    assert!(report.ready);
    assert!(
        !DeliveryJournal::path(&root, "c1").exists(),
        "check must not attach"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn check_marks_tampered_packet_and_bad_ledger_as_not_ready() {
    let root = root("invalid");
    let path = write_packet(&root, "c2", "H");
    delivery::sent(&root, &path, "", 100).unwrap();
    let report = delivery::check(&root, &path, "H", 200).unwrap();
    assert_eq!(report.receipt.status, "incomplete");
    assert!(report.ready);

    let mut changed = packet("c2", "H");
    changed["goal"] = json!("changed bytes");
    fs::write(&path, serde_json::to_vec_pretty(&changed).unwrap()).unwrap();
    let report = delivery::check(&root, &path, "H", 300).unwrap();
    assert_eq!(report.gate, trit_orch::Ternary::True);
    assert_eq!(report.receipt.status, "invalid");
    assert!(!report.ready);

    let bad = write_packet(&root, "c3", "H");
    let ledger = DeliveryJournal::path(&root, "c3");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    fs::write(&ledger, b"{not json").unwrap();
    let report = delivery::check(&root, &bad, "H", 400).unwrap();
    assert_eq!(report.gate, trit_orch::Ternary::True);
    assert_eq!(report.receipt.status, "invalid");
    assert!(!report.ready);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cli_check_returns_gate_and_receipt_without_writing() {
    let root = root("cli");
    let path = write_packet(&root, "c4", "H");
    let (code, value) = run(
        "delivery",
        &json!({
            "action": "check",
            "root": root.to_string_lossy(),
            "packet": path.to_string_lossy(),
            "receiver_state_hash": "H",
            "at": 500,
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["action"], "check");
    assert_eq!(value["check"]["gate"], "True");
    assert_eq!(value["check"]["ready"], true);
    assert_eq!(value["check"]["receipt"]["status"], "missing");
    assert!(
        !DeliveryJournal::path(&root, "c4").exists(),
        "cli check must not attach"
    );
    let _ = fs::remove_dir_all(root);
}
