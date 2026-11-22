//! 锚点的错误密钥与缺失模式。

use std::path::PathBuf;

use trit_orch::{TraceEvent, TraceEventKind, TraceEventStore};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-anchor-strict-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut anchor = path.as_os_str().to_os_string();
    anchor.push(".anchor");
    let _ = std::fs::remove_file(anchor);
    path
}

fn event(seq: u64) -> TraceEvent {
    TraceEvent::new("t", seq, TraceEventKind::TaskSelected, "c", 100 + seq)
}

#[test]
fn wrong_key_and_missing_anchor_are_rejected() {
    let path = temp_path("key");
    let writer = TraceEventStore::with_anchor(&path, b"right-key");
    writer.append(&event(1)).unwrap();
    let error = TraceEventStore::with_anchor(&path, b"wrong-key")
        .load()
        .expect_err("错误密钥必须拒绝");
    assert_eq!(error.kind(), "trace_anchor_mismatch");

    let plain_path = temp_path("plain");
    TraceEventStore::new(&plain_path).append(&event(1)).unwrap();
    // 可选锚点：缺失时退化为无锚点（兼容旧日志）。
    assert!(
        TraceEventStore::with_anchor(&plain_path, b"k")
            .load()
            .is_ok()
    );
    // 严格锚点：非空日志没有锚点文件必须拒绝。
    let error = TraceEventStore::with_anchor_required(&plain_path, b"k")
        .load()
        .expect_err("严格锚点缺失必须拒绝");
    assert_eq!(error.kind(), "trace_anchor_missing");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&plain_path);
}
