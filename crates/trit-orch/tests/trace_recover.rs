//! 半行恢复：截掉不完整尾巴后 load 能继续。

use std::io::Write;
use std::path::PathBuf;

use trit_orch::{TraceEvent, TraceEventKind, TraceEventStore};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-recover-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn event(seq: u64) -> TraceEvent {
    TraceEvent::new("t", seq, TraceEventKind::TaskSelected, "c", 100 + seq)
}

#[test]
fn truncates_half_written_tail() {
    let path = temp_path("half");
    let store = TraceEventStore::new(&path);
    store.append(&event(1)).unwrap();
    store.append(&event(2)).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    write!(file, "{{\"schema\":").unwrap();
    drop(file);

    assert!(store.load().is_err(), "半行尾巴必须先让 load fail-closed");
    let dropped = store.truncate_incomplete_tail().unwrap();
    assert!(dropped > 0);
    assert_eq!(store.load().unwrap().len(), 2);
    let _ = std::fs::remove_file(&path);
}
