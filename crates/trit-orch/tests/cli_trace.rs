//! CLI trace 子命令（JSONL 后端）。

use std::path::PathBuf;

use serde_json::json;
use trit_orch::{TraceEvent, TraceEventKind};

#[allow(dead_code)]
mod cli_support;
use cli_support::*;

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-cli-trace-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut watermark = path.as_os_str().to_os_string();
    watermark.push(".watermark");
    let _ = std::fs::remove_file(watermark);
    path
}

#[test]
fn trace_cli_appends_loads_and_checks() {
    let path = temp_path("jsonl");
    let path_text = path.to_string_lossy().into_owned();
    let event = TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 100);

    let (code, value) = run_with_args(
        "trace",
        &["--trace-backend", "jsonl"],
        &json!({
            "action": "append",
            "path": path_text,
            "events": [serde_json::to_value(&event).unwrap()],
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["appended"], 1);

    let (code, value) = run_with_args(
        "trace",
        &["--trace-backend", "jsonl"],
        &json!({"action": "load", "path": path_text}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["events"].as_array().unwrap().len(), 1);

    let (code, value) = run_with_args(
        "trace",
        &["--trace-backend", "jsonl"],
        &json!({"action": "check", "path": path_text, "trace_id": "t"}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["clean"], true);
    let _ = std::fs::remove_file(&path);
}
