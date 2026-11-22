//! 半行恢复后的重置：水位删除 + 按链尾重封锚点。

use std::io::Write;
use std::path::PathBuf;

use trit_orch::{TraceEvent, TraceEventKind, TraceEventStore};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-reseal-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut anchor = path.as_os_str().to_os_string();
    anchor.push(".anchor");
    let _ = std::fs::remove_file(anchor);
    let mut watermark = path.as_os_str().to_os_string();
    watermark.push(".watermark");
    let _ = std::fs::remove_file(watermark);
    path
}

fn event(seq: u64) -> TraceEvent {
    TraceEvent::new("t", seq, TraceEventKind::TaskSelected, "c", 100 + seq)
}

#[test]
fn truncate_then_reset_watermark_and_reseal_anchor() {
    let path = temp_path("flow");
    let store = TraceEventStore::with_anchor(&path, b"key");
    store.append(&event(1)).unwrap();
    store.append(&event(2)).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    write!(file, "{{\"schema\":").unwrap();
    drop(file);

    assert!(store.load().is_err());
    assert!(store.truncate_incomplete_tail().unwrap() > 0);
    store.reset_watermark().unwrap();
    store.reseal_anchor().unwrap();
    assert_eq!(store.load().unwrap().len(), 2);
    let _ = std::fs::remove_file(&path);
}
