//! 纯 Rust DeliveryReceipt 账本：落盘、四态、actor/证据、包字节钉住、gate attach。
//!
//! 每个用例用独立临时根；账本路径是 `<root>/.agent/delivery/<packet-id>.json`。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use trit_orch::delivery::{self, DeliveryJournal, TOOL};
use trit_orch::{Error, ReceiptState};

fn root(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!(
        "trit-delivery-{tag}-{}-{nanos}",
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
fn sent_then_attach_is_idempotent_and_stays_at_delivered() {
    let root = root("attach");
    let path = write_packet(&root, "p1", "H");
    let journal = delivery::sent(&root, &path, "", 100).unwrap();
    assert_eq!(journal.tool, TOOL);
    assert_eq!(journal.states.len(), 1);
    assert_eq!(journal.state_names(), vec!["sent"]);
    assert_eq!(journal.states[0].actor, "agent-a");
    assert!(DeliveryJournal::path(&root, "p1").is_file());

    let (journal, attached) = delivery::attach(&root, &path, 200).unwrap();
    assert!(attached);
    assert_eq!(journal.state_names(), vec!["sent", "delivered"]);
    assert_eq!(journal.states[1].actor, "agent-b");

    let (journal, attached) = delivery::attach(&root, &path, 300).unwrap();
    assert!(!attached);
    assert_eq!(journal.states.len(), 2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn four_states_require_order_actor_and_acted_evidence() {
    let root = root("states");
    let path = write_packet(&root, "p2", "H");
    delivery::sent(&root, &path, "agent-a", 100).unwrap();
    delivery::attach(&root, &path, 200).unwrap();
    let journal =
        delivery::advance(&root, "p2", ReceiptState::Delivered, "agent-b", "", "", 200).unwrap();
    assert_eq!(journal.states.len(), 2, "repeat delivered is idempotent");

    let error =
        delivery::advance(&root, "p2", ReceiptState::Read, "agent-a", "", "", 300).unwrap_err();
    assert!(
        matches!(error, Error::InputParse { .. }),
        "read must be by the packet receiver: {error}"
    );

    delivery::advance(&root, "p2", ReceiptState::Read, "agent-b", "", "", 300).unwrap();
    let error =
        delivery::advance(&root, "p2", ReceiptState::Acted, "agent-b", " ", "", 400).unwrap_err();
    assert!(
        matches!(error, Error::ReceiptReasonBlank),
        "acted needs evidence: {error}"
    );

    let journal = delivery::advance(
        &root,
        "p2",
        ReceiptState::Acted,
        "agent-b",
        "evidence:commit 6143af9e",
        "",
        400,
    )
    .unwrap();
    assert!(journal.complete());
    assert_eq!(
        journal.state_names(),
        vec!["sent", "delivered", "read", "acted"]
    );
    let report = delivery::verify(&root, "p2", Some(&path)).unwrap();
    assert_eq!(report.status, "complete");
    assert!(report.complete);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skip_and_packet_tampering_are_refused() {
    let root = root("tamper");
    let path = write_packet(&root, "p3", "H");
    delivery::sent(&root, &path, "", 100).unwrap();
    let error =
        delivery::advance(&root, "p3", ReceiptState::Acted, "agent-b", "e", "", 200).unwrap_err();
    assert!(matches!(error, Error::ReceiptSkip { .. }), "{error}");

    let mut changed = packet("p3", "H");
    changed["goal"] = json!("changed bytes");
    fs::write(&path, serde_json::to_vec_pretty(&changed).unwrap()).unwrap();
    let error = delivery::sent(&root, &path, "", 300).unwrap_err();
    assert!(
        matches!(error, Error::InputParse { .. }),
        "tampered bytes must not continue the ledger: {error}"
    );
    let report = delivery::verify(&root, "p3", Some(&path)).unwrap();
    assert_eq!(report.status, "invalid");
    assert_eq!(report.reason, "packet bytes changed since sent");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn gate_attaches_only_when_true() {
    let root = root("gate");
    let path = write_packet(&root, "p4", "H");
    let report = delivery::gate(&root, &path, "not-H", 100).unwrap();
    assert_eq!(report.gate, trit_orch::Ternary::Unknown);
    assert!(!report.attached);
    assert!(report.journal.is_none());
    assert!(!DeliveryJournal::path(&root, "p4").exists());

    let report = delivery::gate(&root, &path, "H", 100).unwrap();
    assert_eq!(report.gate, trit_orch::Ternary::True);
    assert!(report.attached);
    assert_eq!(
        report.journal.unwrap().state_names(),
        vec!["sent", "delivered"]
    );
    let _ = fs::remove_dir_all(root);
}
