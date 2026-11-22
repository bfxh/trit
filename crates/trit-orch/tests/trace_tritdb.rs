#![cfg(feature = "tritdb")]

//! trit-db 后端：事件/元数据事务、重开、锚点密钥。

use std::path::PathBuf;

use trit_orch::tritdb_backend::TritDbTraceStore;
use trit_orch::{TraceEvent, TraceEventKind, TraceLog, check_trace};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("trit-orch-tritdb-{tag}-{}.wal", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn event(seq: u64) -> TraceEvent {
    TraceEvent::new("t", seq, TraceEventKind::TaskSelected, "c", 100 + seq)
}

#[test]
fn append_load_and_reopen() {
    let path = temp_path("roundtrip");
    {
        let store = TritDbTraceStore::open(&path).unwrap();
        store.append(&event(1)).unwrap();
        store.append(&event(2)).unwrap();
        assert_eq!(store.load().unwrap().len(), 2);
    }
    {
        let store = TritDbTraceStore::open(&path).unwrap();
        store.append(&event(3)).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(
            loaded
                .iter()
                .map(|event| event.chain_seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let log = store.load_log("t").unwrap();
        assert!(check_trace(&log).is_clean());
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn wrong_anchor_key_is_rejected() {
    let path = temp_path("anchor");
    {
        let store = TritDbTraceStore::with_anchor(&path, b"right").unwrap();
        store.append(&event(1)).unwrap();
    }
    {
        let store = TritDbTraceStore::with_anchor(&path, b"wrong").unwrap();
        let error = store.load().expect_err("错误密钥必须拒绝");
        assert_eq!(error.kind(), "trace_anchor_mismatch");
    }
    {
        let store = TritDbTraceStore::with_anchor(&path, b"right").unwrap();
        assert_eq!(store.load().unwrap().len(), 1);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn load_log_filters_one_trace() {
    let path = temp_path("filter");
    let store = TritDbTraceStore::open(&path).unwrap();
    store.append(&event(1)).unwrap();
    store
        .append(&TraceEvent::new(
            "other",
            1,
            TraceEventKind::TaskSelected,
            "c",
            200,
        ))
        .unwrap();
    let mut log = TraceLog::new("t");
    for event in store.load().unwrap() {
        if event.trace_id == "t" {
            log.append(event).unwrap();
        }
    }
    assert_eq!(log.len(), 1);
    let _ = std::fs::remove_file(&path);
}
