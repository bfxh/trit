#![cfg(feature = "tritdb-cas")]

//! CAS manifest 快照/恢复轨迹 WAL。

use std::path::PathBuf;

use trit_orch::tritdb_backend::{TritDbTraceStore, cas};
use trit_orch::{TraceEvent, TraceEventKind};

fn temp_path(tag: &str, suffix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-cas-{tag}-{}{}",
        std::process::id(),
        suffix
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn event(seq: u64) -> TraceEvent {
    TraceEvent::new("t", seq, TraceEventKind::TaskSelected, "c", 100 + seq)
}

#[test]
fn snapshot_and_restore_roundtrip() {
    let db = temp_path("db", ".wal");
    let restored = temp_path("restored", ".wal");
    let cas_root = temp_path("cas", "");
    let _ = std::fs::remove_dir_all(&cas_root);

    {
        let store = TritDbTraceStore::open(&db).unwrap();
        store.append(&event(1)).unwrap();
        store.append(&event(2)).unwrap();
    }
    let head = cas::snapshot_wal(&db, &cas_root).unwrap();
    let _ = std::fs::remove_file(&db);

    cas::restore_wal(&restored, &cas_root, &head).unwrap();
    let store = TritDbTraceStore::open(&restored).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].payload_hash, loaded[0].computed_payload_hash());

    let _ = std::fs::remove_file(&restored);
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_dir_all(&cas_root);
}
