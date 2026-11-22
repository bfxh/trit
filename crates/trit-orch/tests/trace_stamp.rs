//! 可选可信写盘时间：store 覆盖调用方填的 recorded_at。

use std::path::PathBuf;

use trit_orch::{TraceEvent, TraceEventKind, TraceEventStore};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-stamp-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn stamped_store_overwrites_caller_recorded_at() {
    let path = temp_path("now");
    let store = TraceEventStore::with_stamped_time(&path);
    let mut event = TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 0);
    event.recorded_at = 1;
    store.append(&event).unwrap();

    let loaded = store.load().unwrap();
    assert!(
        loaded[0].recorded_at > 1,
        "with_stamped_time 必须用宿主时间覆盖调用方值"
    );
    let _ = std::fs::remove_file(&path);
}
