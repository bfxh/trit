//! 旧格式兼容：没有摘要的历史事件按 seq 迁移成真实链。

use std::path::PathBuf;

use trit_orch::{TraceEvent, TraceEventKind, TraceEventStore, TraceLog, check_trace};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-legacy-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn old_event(seq: u64) -> TraceEvent {
    TraceEvent::new("legacy", seq, TraceEventKind::TaskSelected, "c", 100 + seq)
}

fn write_legacy(path: &std::path::Path, events: &[TraceEvent]) {
    let text: String = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap() + "\n")
        .collect();
    std::fs::write(path, text).unwrap();
}

#[test]
fn legacy_file_loads_as_migrated_chain() {
    let path = temp_path("load");
    write_legacy(&path, &[old_event(1), old_event(2)]);
    let store = TraceEventStore::new(&path);
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].chain_seq, 1);
    assert_eq!(loaded[1].chain_seq, 2);
    assert!(!loaded[0].payload_hash.is_empty());
    assert_eq!(loaded[1].prev_hash, loaded[0].payload_hash);

    let mut log = TraceLog::new("legacy");
    for event in loaded {
        log.append(event).unwrap();
    }
    assert!(check_trace(&log).is_clean());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn append_to_legacy_file_migrates_then_links() {
    let path = temp_path("append");
    write_legacy(&path, &[old_event(1), old_event(2)]);
    let store = TraceEventStore::new(&path);
    store.append(&old_event(3)).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(
        loaded
            .iter()
            .map(|event| event.chain_seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    let mut log = TraceLog::new("legacy");
    for event in loaded {
        log.append(event).unwrap();
    }
    assert!(check_trace(&log).is_clean());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn strict_store_refuses_legacy_file() {
    let path = temp_path("strict");
    write_legacy(&path, &[old_event(1)]);
    let error = TraceEventStore::with_strict_hash(&path)
        .load()
        .expect_err("strict 模式必须拒绝旧格式");
    assert_eq!(error.kind(), "trace_hash_missing");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mixed_hash_file_is_rejected() {
    let path = temp_path("mixed");
    let mut sealed = old_event(1);
    sealed.chain_seq = 1;
    sealed.seal("");
    write_legacy(&path, &[sealed, old_event(2)]);
    let error = TraceEventStore::new(&path)
        .load()
        .expect_err("混合格式必须 fail-closed");
    assert_eq!(error.kind(), "trace_hash_missing");
    let _ = std::fs::remove_file(&path);
}
