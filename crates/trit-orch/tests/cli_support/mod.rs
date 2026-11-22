use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

pub(crate) fn run(command: &str, input: &Value) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_trit-orch"))
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("binary should start");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(stdout.trim()).expect("stdout must be json");
    (output.status.code().unwrap_or(-1), value)
}

pub(crate) fn task(id: &str) -> Value {
    json!({
        "task": {"id": id, "priority": 0, "duration": 1, "deps": []},
        "requirements": {"cpu": 1, "memory": 1},
        "max_tokens": 100,
        "epoch": 0
    })
}

pub(crate) fn observation() -> Value {
    json!({
        "phase": "Pending",
        "epoch": 0,
        "authorization": "True",
        "freshness": "True",
        "evidence": null,
        "usage": {"fresh": 0, "cached": 0, "output": 0}
    })
}

pub(crate) fn snapshot(ids: &[&str]) -> Value {
    let mut observations = serde_json::Map::new();
    for id in ids {
        observations.insert((*id).into(), observation());
    }
    json!({"revision": 1, "observations": observations})
}

pub(crate) fn limits() -> Value {
    json!({"workers": 2, "capacity": {"cpu": 2, "memory": 2}, "tokens": 1000})
}

pub(crate) fn run_with_args(command: &str, extra: &[&str], input: &Value) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_trit-orch"))
        .arg(command)
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("binary should start");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let value: Value =
        serde_json::from_str(String::from_utf8(output.stdout).unwrap().trim()).unwrap();
    (output.status.code().unwrap_or(-1), value)
}

pub(crate) fn context_facts(extra: Value) -> Value {
    let mut facts = json!({
        "measured_tokens": 60000,
        "context_window": 100000,
        "threshold_bp": 5100,
        "prunable_tokens": 0,
        "overflow": "not_reported",
        "durable_flushed": "Unknown",
        "recall_reachable": "Unknown",
        "constraints_pinned": "Unknown",
        "progress_since_compaction": "Unknown"
    });
    for (key, value) in extra.as_object().unwrap() {
        facts[key] = value.clone();
    }
    facts
}
