//! 规范化哈希视图必须与旧 clone + 清空 hash 的 JSON 逐字节一致。

use trit_orch::{Resources, Task, TaskSpec, TraceEvent, TraceEventKind, TracePayload};

fn event() -> TraceEvent {
    TraceEvent::new("t", 1, TraceEventKind::PlanPatched, "c", 100).with_payload(
        TracePayload::Plan {
            revision: 3,
            tasks: vec![TaskSpec {
                task: Task {
                    id: "a".into(),
                    priority: 0,
                    duration: 1,
                    deps: vec![],
                },
                requirements: Resources::new(1, 1),
                max_tokens: 100,
                epoch: 0,
            }],
        },
    )
}

#[test]
fn canonical_bytes_match_clone_with_cleared_hashes() {
    let mut event = event();
    event.payload_hash = "old".into();
    event.prev_hash = "prev".into();
    let mut clone = event.clone();
    clone.payload_hash.clear();
    clone.prev_hash.clear();
    assert_eq!(
        event.canonical_bytes(),
        serde_json::to_vec(&clone).unwrap(),
        "免克隆视图必须与旧序列化逐字节一致"
    );
}
