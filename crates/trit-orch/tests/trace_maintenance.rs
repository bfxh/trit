#![cfg(all(feature = "tritdb-cas", feature = "tritdb-sql"))]

//! 维护命令：CAS 链压缩与物化表 GC 的行为。

use std::path::PathBuf;

use trit_orch::tritdb_backend::{TritDbTraceStore, cas, sqlite_gc, sqlite_materialize};
use trit_orch::{TraceEvent, TraceEventKind};

fn temp_path(tag: &str, suffix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("trit-maint-{tag}-{}{suffix}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn event(seq: u64) -> TraceEvent {
    TraceEvent::new("t", seq, TraceEventKind::TaskSelected, "c", 100 + seq)
}

fn append(db: &std::path::Path, seqs: impl IntoIterator<Item = u64>) {
    let store = TritDbTraceStore::open(db).unwrap();
    for seq in seqs {
        store.append(&event(seq)).unwrap();
    }
}

#[test]
fn compact_collapses_layers_and_restore_still_works() {
    let db = temp_path("db", ".wal");
    let restored = temp_path("restored", ".wal");
    let cas_root = temp_path("cas", "");
    let _ = std::fs::remove_dir_all(&cas_root);
    let key = b"maint-key";

    append(&db, [1, 2]);
    let first = cas::snapshot_wal_with_key(&db, &cas_root, Some(key)).unwrap();
    append(&db, [3]);
    let second = cas::snapshot_wal_with_key(&db, &cas_root, Some(key)).unwrap();
    assert_eq!(second.parent.as_deref(), Some(first.head.as_str()));

    let info = cas::compact(&cas_root, None, Some(key)).unwrap();
    assert_eq!(info.layers, 2, "压缩前应为两层");
    assert_eq!(info.previous.as_deref(), Some(second.head.as_str()));
    assert_ne!(info.head, second.head);

    // 压缩后 restore 仍能恢复出全部 3 条
    cas::restore_wal_from(&restored, &cas_root, None, Some(key)).unwrap();
    let loaded = TritDbTraceStore::open(&restored).unwrap().load().unwrap();
    assert_eq!(loaded.len(), 3);

    // 幂等：再压缩一次不再产生新层
    let again = cas::compact(&cas_root, None, Some(key)).unwrap();
    assert_eq!(again.layers, 1);
    assert!(again.previous.is_none());

    for path in [&db, &restored] {
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_dir_all(&cas_root);
}

#[test]
fn gc_keeps_recent_rows_and_sync_does_not_backfill() {
    let db = temp_path("gc", ".wal");
    let sqlite = db.with_extension("sqlite");
    for path in [&db, &sqlite] {
        let _ = std::fs::remove_file(path);
    }

    append(&db, 1..=10);
    let first = sqlite_materialize::sync(&db, &sqlite).unwrap();
    assert_eq!(first.inserted, 10);
    assert_eq!(first.watermark, 10);

    let report = sqlite_gc::gc(&sqlite, 3).unwrap();
    assert_eq!(report.deleted, 7);
    assert_eq!(report.total_after, 3);

    // 关键：watermark 所在行被保留，sync 不会把删掉的行插回来
    let second = sqlite_materialize::sync(&db, &sqlite).unwrap();
    assert_eq!(second.inserted, 0, "GC 后不应回填已删除的行");
    assert_eq!(second.watermark, 10);

    // 新事件仍能正常追上
    append(&db, [11]);
    let third = sqlite_materialize::sync(&db, &sqlite).unwrap();
    assert_eq!(third.inserted, 1);
    assert_eq!(third.watermark, 11);

    for path in [&db, &sqlite] {
        let _ = std::fs::remove_file(path);
    }
}
