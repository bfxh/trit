//! 链头锚点：整链重算被锚点抓到。

use std::path::PathBuf;

use trit_orch::{TraceEvent, TraceEventKind, TraceEventStore};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-anchor-{tag}-{}.jsonl",
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

fn anchor_path(path: &std::path::Path) -> PathBuf {
    let mut anchor = path.as_os_str().to_os_string();
    anchor.push(".anchor");
    PathBuf::from(anchor)
}

#[test]
fn whole_chain_rewrite_is_detected_by_anchor() {
    let path = temp_path("rewrite");
    let key = b"anchor-key";
    let store = TraceEventStore::with_anchor(&path, key);
    for seq in 1..=3 {
        store.append(&event(seq)).unwrap();
    }
    assert!(anchor_path(&path).exists());
    assert_eq!(store.load().unwrap().len(), 3);

    // 模拟能改整个文件的攻击者：改字段、重算内容摘要与整条链。
    let mut events = store.load().unwrap();
    for event in &mut events {
        event.observed_at += 10;
    }
    let mut previous = String::new();
    for (index, event) in events.iter_mut().enumerate() {
        event.chain_seq = (index + 1) as u64;
        event.seal(&previous);
        previous = event.payload_hash.clone();
    }
    let text: String = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap() + "\n")
        .collect();
    std::fs::write(&path, text).unwrap();

    let error = TraceEventStore::with_anchor(&path, key)
        .load()
        .expect_err("整链重算必须被锚点抓到");
    assert_eq!(error.kind(), "trace_anchor_mismatch");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(anchor_path(&path));
}
