//! shadow TraceStore 的整文件 HMAC 锚点。

use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_orch::{Ledger, Limits, Resources, Snapshot, TraceStep, TraceStore};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-shadow-anchor-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut anchor = path.as_os_str().to_os_string();
    anchor.push(".shadow-anchor");
    let _ = std::fs::remove_file(anchor);
    path
}

fn step() -> TraceStep {
    TraceStep {
        revision: 1,
        tasks: Vec::new(),
        snapshot: Snapshot {
            revision: 1,
            observations: BTreeMap::new(),
        },
        limits: Limits {
            workers: 1,
            capacity: Resources::new(1, 1),
            tokens: 10,
        },
        ledger: Ledger::new(),
    }
}

#[test]
fn anchor_detects_file_replacement_and_wrong_key() {
    let path = temp_path("verify");
    let store = TraceStore::with_anchor(&path, b"key");
    store.append(&step()).unwrap();
    assert_eq!(store.load().unwrap().len(), 1);

    // 追加一行（模拟整个文件被改）后，整文件摘要必须失配。
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    use std::io::Write;
    writeln!(file, "{}", serde_json::to_string(&step()).unwrap()).unwrap();
    drop(file);
    let error = store.load().expect_err("整文件替换必须被抓到");
    assert_eq!(error.kind(), "trace_anchor_mismatch");

    let error = TraceStore::with_anchor(&path, b"wrong")
        .load()
        .expect_err("错误密钥必须拒绝");
    assert_eq!(error.kind(), "trace_anchor_mismatch");

    let plain = temp_path("plain");
    TraceStore::new(&plain).append(&step()).unwrap();
    assert!(TraceStore::with_anchor(&plain, b"k").load().is_ok());
    let error = TraceStore::with_anchor_required(&plain, b"k")
        .load()
        .expect_err("严格锚点缺失必须拒绝");
    assert_eq!(error.kind(), "trace_anchor_missing");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&plain);
}
