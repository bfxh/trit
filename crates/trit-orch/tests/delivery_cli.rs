//! `trit-orch delivery` CLI 的进程级回归：stdin JSON、stdout JSON、退出码。
//!
//! 每个用例用独立临时根；CLI 的 `root` 是账本根，`packet` 是交接包文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

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
        "trit-delivery-cli-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn packet(id: &str) -> Value {
    json!({
        "id": id,
        "from": "agent-a",
        "to": "agent-b",
        "goal": "hand off the ledger",
        "base_commit": "6143af9e",
        "state_hash": "H",
        "decisions": ["keep it in rust"],
        "open_questions": [],
        "claims": [],
        "evidence": [],
        "next_action": "read the ledger",
        "idempotency_key": format!("{id}-key"),
        "expires_at": 10_000_000_000u64,
    })
}

fn write_packet(root: &Path, id: &str) -> PathBuf {
    let path = root.join("packet.json");
    fs::write(&path, serde_json::to_vec_pretty(&packet(id)).unwrap()).unwrap();
    path
}

#[test]
fn cli_round_trip_writes_ledger_under_root() {
    let root = root("cli");
    let path = write_packet(&root, "p5");
    let root_text = root.to_string_lossy().to_string();
    let path_text = path.to_string_lossy().to_string();

    let (code, value) = run(
        "delivery",
        &json!({
            "action": "sent",
            "root": root_text,
            "packet": path_text,
            "at": 100,
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["states"], json!(["sent"]));
    assert_eq!(value["action"], "sent");

    let (code, value) = run(
        "delivery",
        &json!({
            "action": "gate",
            "root": root.to_string_lossy(),
            "packet": path.to_string_lossy(),
            "receiver_state_hash": "H",
            "at": 200,
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "True");
    assert_eq!(value["attached"], true);
    assert_eq!(value["states"], json!(["sent", "delivered"]));

    let (code, value) = run(
        "delivery",
        &json!({
            "action": "verify",
            "root": root.to_string_lossy(),
            "packet_id": "p5",
            "packet": path.to_string_lossy(),
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["verify"]["status"], "incomplete");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cli_advance_uses_state_field_and_stops_without_acted_evidence() {
    let root = root("cli-advance");
    let path = write_packet(&root, "p6");
    let root_text = root.to_string_lossy().to_string();
    let path_text = path.to_string_lossy().to_string();
    run(
        "delivery",
        &json!({
            "action": "sent",
            "root": root_text,
            "packet": path_text,
            "at": 100,
        }),
    );
    run(
        "delivery",
        &json!({
            "action": "attach",
            "root": root.to_string_lossy(),
            "packet": path.to_string_lossy(),
            "at": 200,
        }),
    );
    let (code, value) = run(
        "delivery",
        &json!({
            "action": "advance",
            "root": root.to_string_lossy(),
            "packet_id": "p6",
            "state": "Read",
            "actor": "agent-b",
            "at": 300,
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["states"], json!(["sent", "delivered", "read"]));

    let (code, value) = run(
        "delivery",
        &json!({
            "action": "advance",
            "root": root.to_string_lossy(),
            "packet_id": "p6",
            "state": "Acted",
            "actor": "agent-b",
            "at": 400,
        }),
    );
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["kind"], "receipt_reason_blank", "{value}");

    let (code, value) = run(
        "delivery",
        &json!({
            "action": "advance",
            "root": root.to_string_lossy(),
            "packet_id": "p6",
            "state": "Acted",
            "actor": "agent-b",
            "evidence": "commit:rust",
            "at": 400,
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        value["states"],
        json!(["sent", "delivered", "read", "acted"])
    );
    let _ = fs::remove_dir_all(root);
}
