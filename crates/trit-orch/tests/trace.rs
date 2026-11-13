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

fn expect_err(
    result: Result<Vec<TraceStep>, trit_orch::Error>,
    diagnostic: &str,
) -> trit_orch::Error {
    match result {
        Ok(_) => panic!("{diagnostic}: 期望错误却成功了"),
        Err(error) => error,
    }
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
        ..TaskSpec::default()
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
    assert!(
        store
            .load()
            .expect("ORCH_TRACE_MISSING_FILE: 缺文件应为空")
            .is_empty(),
        "ORCH_TRACE_MISSING_FILE"
    );
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
    let error = expect_err(store.load(), "ORCH_TRACE_PARSE");
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
    let error = expect_err(store.load(), "ORCH_TRACE_PARSE");
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

#[test]
fn empty_file_is_an_empty_trace() {
    let path = temp_path("empty");
    std::fs::write(&path, "").unwrap();
    let store = TraceStore::new(&path);
    assert!(store.load().unwrap().is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn whitespace_only_lines_are_skipped() {
    let path = temp_path("whitespace");
    let store = TraceStore::new(&path);
    store.append(&step("a", 1)).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("   \n\t\n \t \n");
    std::fs::write(&path, text).unwrap();
    let loaded = store
        .load()
        .unwrap_or_else(|error| panic!("ORCH_TRACE_WHITESPACE: 空白行不该导致解析失败: {error}"));
    assert_eq!(loaded.len(), 1, "ORCH_TRACE_WHITESPACE");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn crlf_line_endings_are_accepted() {
    let path = temp_path("crlf");
    let store = TraceStore::new(&path);
    let line = serde_json::to_string(&step("a", 1)).unwrap();
    std::fs::write(&path, format!("{line}\r\n{line}\r\n")).unwrap();
    let loaded = store
        .load()
        .unwrap_or_else(|error| panic!("ORCH_TRACE_CRLF: CRLF 应被接受: {error}"));
    assert_eq!(loaded.len(), 2, "ORCH_TRACE_CRLF");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn first_bad_line_wins_even_if_later_lines_are_also_bad() {
    let path = temp_path("first-bad");
    let store = TraceStore::new(&path);
    store.append(&step("a", 1)).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{oops}\n{also bad}\n");
    std::fs::write(&path, text).unwrap();
    let error = expect_err(store.load(), "ORCH_TRACE_PARSE");
    assert_eq!(error.kind(), "trace_parse");
    assert!(
        error.to_string().contains(":2:"),
        "ORCH_TRACE_PARSE: 只报第一条坏行 {error}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn truncated_last_line_reports_its_number() {
    let path = temp_path("truncated");
    let line = serde_json::to_string(&step("a", 1)).unwrap();
    let truncated = &line[..line.len() / 2];
    std::fs::write(&path, format!("{line}\n{truncated}")).unwrap();
    let store = TraceStore::new(&path);
    let error = expect_err(store.load(), "ORCH_TRACE_PARSE");
    assert!(
        error.to_string().contains(":2:"),
        "ORCH_TRACE_PARSE: 截断的末行 {error}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn valid_json_with_wrong_shape_is_a_parse_error() {
    let path = temp_path("shape");
    std::fs::write(&path, "{}\n").unwrap();
    let error = expect_err(TraceStore::new(&path).load(), "ORCH_TRACE_SHAPE");
    assert_eq!(error.kind(), "trace_parse");
    assert!(
        error.to_string().contains(":1:"),
        "ORCH_TRACE_SHAPE: {error}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn non_utf8_bytes_report_io_not_a_silent_empty_trace() {
    let path = temp_path("non-utf8");
    std::fs::write(&path, [0xff, 0xfe, 0x00, 0x7b]).unwrap();
    let error = expect_err(TraceStore::new(&path).load(), "ORCH_TRACE_SHAPE");
    assert_eq!(error.kind(), "trace_io", "ORCH_TRACE_NON_UTF8");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_directory_at_the_trace_path_is_an_io_error() {
    let path = temp_path("dir");
    std::fs::create_dir_all(&path).unwrap();
    let error = expect_err(TraceStore::new(&path).load(), "ORCH_TRACE_SHAPE");
    assert_eq!(error.kind(), "trace_io", "ORCH_TRACE_DIR");
    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn appending_the_same_step_twice_keeps_both() {
    let path = temp_path("duplicate");
    let store = TraceStore::new(&path);
    let one = step("a", 1);
    store.append(&one).unwrap();
    store.append(&one).unwrap();
    store.append(&step("b", 2)).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 3, "轨迹是日志，不去重");
    assert_eq!(loaded[0], loaded[1]);
    assert_eq!(loaded[2].revision, 2);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn unicode_identifiers_roundtrip_byte_exact() {
    let path = temp_path("unicode");
    let store = TraceStore::new(&path);
    let original = step("调度-单元-①", 7);
    store.append(&original).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0], original, "ORCH_TRACE_UNICODE");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn large_step_roundtrips() {
    let path = temp_path("large");
    let store = TraceStore::new(&path);
    let mut big = step("t0000", 1);
    big.tasks = (0..2_000)
        .map(|index| {
            let mut spec = big.tasks[0].clone();
            spec.task.id = format!("t{index:04}");
            spec
        })
        .collect();
    store.append(&big).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded[0].tasks.len(), 2_000, "ORCH_TRACE_LARGE");
    assert_eq!(loaded[0].tasks[1_999].task.id, "t1999");
    let _ = std::fs::remove_file(&path);
}
