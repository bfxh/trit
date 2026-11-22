//! 多 trace 共用同一个 store：单 trace 过滤后的写入序缺口不能误报断链。

use std::path::PathBuf;

use trit_orch::{TraceEvent, TraceEventKind, TraceEventStore, check_trace};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-multi-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn per_trace_check_tolerates_global_chain_gaps() {
    let path = temp_path("gap");
    let store = TraceEventStore::new(&path);
    for seq in 1..=3u64 {
        store
            .append(&TraceEvent::new(
                "a",
                seq,
                TraceEventKind::TaskSelected,
                "c",
                100 + seq,
            ))
            .unwrap();
        store
            .append(&TraceEvent::new(
                "b",
                seq,
                TraceEventKind::TaskSelected,
                "c",
                100 + seq,
            ))
            .unwrap();
    }

    let log = store.load_log("a").unwrap();
    assert_eq!(log.len(), 3);
    assert!(
        check_trace(&log).is_clean(),
        "多 trace 的写入序缺口不能被误报为断链"
    );
    let _ = std::fs::remove_file(&path);
}
