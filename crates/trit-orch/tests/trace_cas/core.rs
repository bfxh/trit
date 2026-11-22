//! 快照/恢复的主路径：parent 链、anchor 落盘、错误 key 被拒。

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
fn snapshot_parent_chain_anchor_and_restore() {
    let db = temp_path("db", ".wal");
    let restored = temp_path("restored", ".wal");
    let cas_root = temp_path("cas", "");
    let _ = std::fs::remove_dir_all(&cas_root);

    {
        let store = TritDbTraceStore::open(&db).unwrap();
        store.append(&event(1)).unwrap();
        store.append(&event(2)).unwrap();
    }
    let key = b"cas-key";
    let first = cas::snapshot_wal_with_key(&db, &cas_root, Some(key)).unwrap();
    let second = cas::snapshot_wal_with_key(&db, &cas_root, Some(key)).unwrap();
    assert_eq!(second.parent.as_deref(), Some(first.head.as_str()));
    assert!(std::path::Path::new(&second.anchor).exists());

    let _ = std::fs::remove_file(&db);
    let restored_head = cas::restore_wal_from(&restored, &cas_root, None, Some(key)).unwrap();
    assert_eq!(restored_head, second.head);
    let store = TritDbTraceStore::open(&restored).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].payload_hash, loaded[0].computed_payload_hash());

    let error = cas::restore_wal_from(&restored, &cas_root, None, Some(b"wrong"))
        .expect_err("错误 key 必须拒绝 anchor");
    assert_eq!(error.kind(), "trace_anchor_mismatch");

    let _ = std::fs::remove_file(&restored);
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_dir_all(&cas_root);
}
