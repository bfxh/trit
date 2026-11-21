//! 轨迹存储与折叠：schema 闸门、坏行行号、乱序归位、旧步骤迁移。
//!
//! 这一层是审计可信度的地基：写进去的事件如果被静默丢弃、或者坏行被跳过，
//! 后面所有的"回放出当时决定"都会建立在一份残缺日志上，而且**看不出来**。

use std::collections::BTreeMap;

use trit_orch::{
    AppendOutcome, Error, EventId, Ledger, Limits, LogicalTime, Observation, Phase, Resources,
    Snapshot, Task, TaskSpec, Ternary, TraceEvent, TraceEventKind, TraceEventStore, TraceLog,
    TracePayload, TraceStep, events_from_step,
};

fn task(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
    }
}

fn step(revision: u64, ids: &[&str]) -> TraceStep {
    let tasks: Vec<TaskSpec> = ids.iter().map(|id| task(id)).collect();
    let observations = ids
        .iter()
        .map(|id| {
            (
                (*id).to_string(),
                Observation {
                    phase: Phase::Pending,
                    authorization: Ternary::True,
                    freshness: Ternary::True,
                    ..Observation::default()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    TraceStep {
        revision,
        tasks,
        snapshot: Snapshot {
            revision,
            observations,
        },
        limits: Limits {
            workers: 4,
            capacity: Resources::new(4, 4),
            tokens: 10_000,
        },
        ledger: Ledger::new(),
    }
}

fn event(trace: &str, seq: u64, kind: TraceEventKind) -> TraceEvent {
    TraceEvent::new(trace, seq, kind, "c1", 100 + seq)
}

#[test]
fn missing_file_loads_as_empty_not_as_an_error() {
    let store = TraceEventStore::new(std::env::temp_dir().join("orch-trace-absent-xyz.jsonl"));
    let _ = std::fs::remove_file(store.path());
    assert_eq!(store.load().expect("missing file is empty"), Vec::new());
}

#[test]
fn append_then_load_round_trips_payloads() {
    let path = std::env::temp_dir().join("orch-trace-roundtrip.jsonl");
    let _ = std::fs::remove_file(&path);
    let store = TraceEventStore::new(&path);

    let mut first = event("t1", 1, TraceEventKind::PlanPatched);
    first = first.with_payload(TracePayload::Plan {
        revision: 3,
        tasks: vec![task("a"), task("b")],
    });
    let second =
        event("t1", 2, TraceEventKind::SnapshotCaptured).with_payload(TracePayload::Snapshot {
            snapshot: Snapshot {
                revision: 3,
                observations: BTreeMap::new(),
            },
        });

    store.append(&first).expect("append");
    store.append(&second).expect("append");

    let loaded = store.load().expect("load");
    assert_eq!(loaded.len(), 2, "ORCH_TRACE_STORE_ROUNDTRIP");
    assert_eq!(loaded[0], first);
    assert_eq!(loaded[1], second);
}

#[test]
fn append_creates_missing_parent_directories() {
    let dir = std::env::temp_dir().join("orch-trace-nested/deeper");
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("orch-trace-nested"));
    let store = TraceEventStore::new(dir.join("log.jsonl"));
    store
        .append(&event("t2", 1, TraceEventKind::PlanPatched))
        .expect("parent dirs created");
    assert!(store.path().exists(), "ORCH_TRACE_STORE_MKDIR");
}

#[test]
fn stale_schema_is_refused_on_write_and_on_read() {
    // schema 是"这份日志能不能被信任解释"的唯一声明。写旧版本的日志必须当场失败。
    let path = std::env::temp_dir().join("orch-trace-schema.jsonl");
    let _ = std::fs::remove_file(&path);
    let store = TraceEventStore::new(&path);

    let mut stale = event("t3", 1, TraceEventKind::PlanPatched);
    stale.schema = 0;
    let error = store.append(&stale).expect_err("stale write refused");
    match error {
        Error::TraceSchema { found, expected } => {
            assert_eq!(found, 0);
            assert_eq!(
                expected,
                TraceEvent::CURRENT_SCHEMA,
                "ORCH_TRACE_STORE_SCHEMA"
            );
        }
        other => panic!("expected TraceSchema, got {}", other.kind()),
    }
    assert!(!path.exists(), "拒绝的事件不能留下空文件");

    // 从一条**合法**序列化结果出发，只把 schema 改成旧版本：这样失败只可能来自 schema 闸，
    // 不会混进"JSON 本来就写错了"。
    let valid =
        serde_json::to_string(&event("t3", 1, TraceEventKind::PlanPatched)).expect("serialize");
    std::fs::write(
        &path,
        format!("{}\n", valid.replace("\"schema\":1", "\"schema\":0")),
    )
    .expect("write");
    let error = store.load().expect_err("stale read refused");
    assert_eq!(error.kind(), "trace_schema", "ORCH_TRACE_STORE_SCHEMA_READ");
}

#[test]
fn bad_line_reports_one_based_line_number() {
    // 行号是排障时唯一的坐标。报 0 或报 1-based 错位都会让"去那行看看"变成无用功。
    let path = std::env::temp_dir().join("orch-trace-badline.jsonl");
    let _ = std::fs::remove_file(&path);
    let store = TraceEventStore::new(&path);
    store
        .append(&event("t4", 1, TraceEventKind::PlanPatched))
        .expect("first line ok");
    std::fs::write(
        &path,
        format!(
            "{}\n{{\"nope\": true}}\n",
            std::fs::read_to_string(&path).expect("read").trim_end()
        ),
    )
    .expect("rewrite");
    let error = store.load().expect_err("bad line refused");
    match error {
        Error::TraceParse { line, .. } => {
            assert_eq!(line, 2, "ORCH_TRACE_STORE_BADLINE");
        }
        other => panic!("expected TraceParse, got {}", other.kind()),
    }
}

#[test]
fn load_log_only_takes_the_requested_trace() {
    let path = std::env::temp_dir().join("orch-trace-filter.jsonl");
    let _ = std::fs::remove_file(&path);
    let store = TraceEventStore::new(&path);
    store
        .append(&event("keep", 1, TraceEventKind::PlanPatched))
        .unwrap();
    store
        .append(&event("drop", 1, TraceEventKind::PlanPatched))
        .unwrap();
    store
        .append(&event("keep", 2, TraceEventKind::SnapshotCaptured))
        .unwrap();

    let log = store.load_log("keep").expect("load_log");
    assert_eq!(log.len(), 2, "ORCH_TRACE_STORE_FILTER");
    assert_eq!(log.trace_id(), "keep");
}

#[test]
fn out_of_order_file_is_reordered_by_seq() {
    // 文件里的物理顺序不等于逻辑顺序（并发追加会乱）。重建成日志时必须按 seq 归位。
    let path = std::env::temp_dir().join("orch-trace-ooo.jsonl");
    let _ = std::fs::remove_file(&path);
    let store = TraceEventStore::new(&path);
    store
        .append(&event("t5", 3, TraceEventKind::LedgerCaptured))
        .unwrap();
    store
        .append(&event("t5", 1, TraceEventKind::PlanPatched))
        .unwrap();
    store
        .append(&event("t5", 2, TraceEventKind::SnapshotCaptured))
        .unwrap();

    let log = store.load_log("t5").expect("load_log");
    let seqs: Vec<u64> = log.events().map(|event| event.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3], "ORCH_TRACE_STORE_REORDER");
}

#[test]
fn duplicate_event_ids_are_deduplicated_not_double_counted() {
    let mut log = TraceLog::new("t6");
    let first = event("t6", 1, TraceEventKind::PlanPatched);
    assert_eq!(
        log.append(first.clone()).expect("append"),
        AppendOutcome::Recorded
    );
    assert_eq!(
        log.append(first).expect("duplicate append is ok"),
        AppendOutcome::Duplicate,
        "ORCH_TRACE_DEDUP"
    );
    assert_eq!(log.len(), 1);
}

#[test]
fn seq_collision_with_a_different_id_is_an_error() {
    // 同 seq 不同 id 意味着两条互斥事实在抢同一个位置，不能靠"后到覆盖先到"糊过去。
    let mut log = TraceLog::new("t7");
    log.append(event("t7", 1, TraceEventKind::PlanPatched))
        .unwrap();
    let mut clash = event("t7", 1, TraceEventKind::SnapshotCaptured);
    clash.id = "different".into();
    let error = log.append(clash).expect_err("seq conflict");
    assert_eq!(
        error.kind(),
        "trace_seq_conflict",
        "ORCH_TRACE_SEQ_CONFLICT"
    );
}

#[test]
fn late_arrival_is_flagged_so_a_reader_can_tell() {
    let mut log = TraceLog::new("t8");
    log.append(event("t8", 2, TraceEventKind::SnapshotCaptured))
        .unwrap();
    let outcome = log
        .append(event("t8", 1, TraceEventKind::PlanPatched))
        .unwrap();
    assert_eq!(outcome, AppendOutcome::LateButRecorded, "ORCH_TRACE_LATE");
}

#[test]
fn gaps_report_interior_and_leading_holes_but_never_guess_the_tail() {
    let mut log = TraceLog::new("t9");
    for seq in [2u64, 3, 5] {
        log.append(event("t9", seq, TraceEventKind::PlanPatched))
            .unwrap();
    }
    assert_eq!(log.gaps(), vec![(1, 1), (4, 4)], "ORCH_TRACE_GAPS");
}

#[test]
fn legacy_step_becomes_four_ordered_state_events() {
    // 迁移路径必须把一步拆成四条完整状态，缺一条折叠就会判"不完整"。
    let step = step(5, &["a"]);
    let events = events_from_step("t10", 10, &step, 99);
    assert_eq!(events.len(), 4, "ORCH_TRACE_MIGRATE_COUNT");
    let kinds: Vec<&str> = events.iter().map(|event| event.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "plan_patched",
            "snapshot_captured",
            "limits_captured",
            "ledger_captured"
        ],
        "ORCH_TRACE_MIGRATE_KINDS"
    );
    let seqs: Vec<u64> = events.iter().map(|event| event.seq).collect();
    assert_eq!(seqs, vec![10, 11, 12, 13], "ORCH_TRACE_MIGRATE_SEQ");
    assert!(events.iter().all(|event| event.trace_id == "t10"));
    assert!(
        events
            .iter()
            .all(|event| event.correlation_id == "legacy-step:5"),
        "ORCH_TRACE_MIGRATE_CORRELATION"
    );
}

#[test]
fn folded_state_survives_a_round_trip_through_disk() {
    let path = std::env::temp_dir().join("orch-trace-fold.jsonl");
    let _ = std::fs::remove_file(&path);
    let store = TraceEventStore::new(&path);
    store
        .append_step("t11", 1, &step(9, &["a", "b"]), 42)
        .expect("append_step");

    let log = store.load_log("t11").expect("load_log");
    let folded = log.fold();
    assert_eq!(folded.revision, Some(9), "ORCH_TRACE_FOLD");
    assert_eq!(folded.tasks.as_ref().map(|tasks| tasks.len()), Some(2));
    assert!(folded.snapshot.is_some());
    assert!(folded.limits.is_some());
    assert!(folded.ledger.is_some());
}

#[test]
fn latest_step_requires_plan_snapshot_and_limits() {
    let mut log = TraceLog::new("t12");
    log.append(event("t12", 1, TraceEventKind::PlanPatched))
        .unwrap();
    let error = log.latest_step().expect_err("incomplete");
    assert_eq!(error.kind(), "trace_incomplete", "ORCH_TRACE_INCOMPLETE");
}

#[test]
fn event_time_is_kept_separate_from_logical_time() {
    // 墙钟与逻辑时钟混用，会让"同一毫秒里的两件事"失去先后。
    // 事实发生时间未知就是 None，不许拿观察时间顶替。
    let ev = event("t13", 1, TraceEventKind::PlanPatched)
        .with_logical(LogicalTime {
            revision: 3,
            round: 2,
            ..LogicalTime::default()
        })
        .with_event_time(500);
    assert_eq!(ev.event_time, Some(500));
    assert_eq!(ev.observed_at, 101, "观察时间与发生时间是两个字段");
    assert_eq!(ev.logical.revision, 3, "ORCH_TRACE_CLOCKS");
    assert_eq!(ev.logical.round, 2);
    assert_eq!(
        event("t13", 2, TraceEventKind::PlanPatched).event_time,
        None,
        "未知就是 None，不是 0"
    );
}

#[test]
fn event_id_exposes_the_coordinates_it_was_built_from() {
    let id: EventId = "t14:1:plan_patched".to_string();
    let ev = event("t14", 1, TraceEventKind::PlanPatched).with_id(id.clone());
    assert_eq!(ev.id, id);
}
