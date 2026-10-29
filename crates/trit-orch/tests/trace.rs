use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_orch::{
    Limits, Observation, Resources, Snapshot, Task, TaskSpec, Ternary, TraceStep, TraceStore,
};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-trace-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn step(id: &str, revision: u64) -> TraceStep {
    let spec = TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
    };
    TraceStep {
        revision,
        tasks: vec![spec],
        snapshot: Snapshot {
            revision,
            observations: BTreeMap::from([(
                id.to_string(),
                Observation {
                    authorization: Ternary::True,
                    freshness: Ternary::True,
                    ..Observation::default()
                },
            )]),
        },
        limits: Limits {
            workers: 1,
            capacity: Resources::new(1, 1),
            tokens: 1000,
        },
        ledger: Default::default(),
    }
}

#[test]
fn append_and_load_roundtrip_preserves_order() {
    let path = temp_path("roundtrip");
    let store = TraceStore::new(&path);
    assert!(store.load().unwrap().is_empty(), "缺文件视为空");
    store.append(&step("a", 1)).unwrap();
    store.append(&step("b", 2)).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 2, "ORCH_TRACE_ROUNDTRIP");
    assert_eq!(loaded[0].revision, 1);
    assert_eq!(loaded[1].revision, 2);
    assert_eq!(loaded[1].tasks[0].task.id, "b");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn malformed_line_reports_its_line_number() {
    let path = temp_path("malformed");
    let store = TraceStore::new(&path);
    store.append(&step("a", 1)).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{not json}\n");
    std::fs::write(&path, text).unwrap();
    let error = store.load().unwrap_err();
    assert_eq!(error.kind(), "trace_parse", "ORCH_TRACE_PARSE");
    assert!(
        error.to_string().contains(":2:"),
        "ORCH_TRACE_PARSE: {error}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn blank_lines_are_skipped_but_do_not_shift_numbers() {
    let path = temp_path("blank");
    let store = TraceStore::new(&path);
    store.append(&step("a", 1)).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push('\n');
    text.push_str("{oops}\n");
    std::fs::write(&path, text).unwrap();
    let error = store.load().unwrap_err();
    assert!(
        error.to_string().contains(":3:"),
        "ORCH_TRACE_PARSE: 空行要占行号 {error}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn append_creates_missing_directories() {
    let mut path = std::env::temp_dir();
    path.push(format!("trit-orch-trace-dir-{}", std::process::id()));
    path.push("nested");
    path.push("traces.jsonl");
    let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    let store = TraceStore::new(&path);
    store.append(&step("a", 1)).unwrap();
    assert_eq!(store.load().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
}
