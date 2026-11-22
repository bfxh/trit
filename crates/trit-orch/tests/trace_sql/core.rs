//! 增量物化：重复查询不重复插入，新事件能被追上。

use std::path::PathBuf;

use trit_orch::tritdb_backend::TritDbTraceStore;
use trit_orch::tritdb_backend::sql::query_trace_events;
use trit_orch::{TraceEvent, TraceEventKind};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("trit-orch-sql-{tag}-{}.wal", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn event(seq: u64) -> TraceEvent {
    TraceEvent::new("t", seq, TraceEventKind::TaskSelected, "c", 100 + seq)
}

fn count(path: &std::path::Path) -> String {
    let results = query_trace_events(path, "SELECT count(*) AS n FROM trit_trace").unwrap();
    results[0].rows[0][0].clone()
}

#[test]
fn sql_can_count_and_group_events() {
    let path = temp_path("query");
    let store = TritDbTraceStore::open(&path).unwrap();
    store.append(&event(1)).unwrap();
    store.append(&event(2)).unwrap();
    drop(store);

    // 第二次查询不应重复物化。
    assert_eq!(count(&path), "2");
    assert_eq!(count(&path), "2");

    let store = TritDbTraceStore::open(&path).unwrap();
    store.append(&event(3)).unwrap();
    drop(store);
    assert_eq!(count(&path), "3");

    let results = query_trace_events(
        &path,
        "SELECT kind, count(*) AS n FROM trit_trace GROUP BY kind",
    )
    .unwrap();
    assert_eq!(results[0].rows[0][0], "task_selected");
    assert_eq!(results[0].rows[0][1], "3");
    let _ = std::fs::remove_file(&path);
}
