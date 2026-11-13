use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_orch::{
    Error, Ledger, Limits, Observation, Plan, Resources, Snapshot, Task, TaskSpec, Ternary,
    TraceEvent, TraceEventKind, TraceEventStore, TraceLog, TracePayload, TraceStep,
    events_from_step,
};

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-event-store-{tag}-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn replay_step() -> TraceStep {
    let tasks = vec![TaskSpec {
        task: Task {
            id: "a".into(),
            priority: 0,
            duration: 1,
            deps: Vec::new(),
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
        ..TaskSpec::default()
    }];
    let snapshot = Snapshot {
        revision: 7,
        observations: BTreeMap::from([(
            "a".into(),
            Observation {
                authorization: Ternary::True,
                freshness: Ternary::True,
                ..Observation::default()
            },
        )]),
    };
    let limits = Limits {
        workers: 1,
        capacity: Resources::new(1, 1),
        tokens: 1_000,
    };
    TraceStep {
        revision: 7,
        tasks,
        snapshot,
        limits,
        ledger: Ledger::new(),
    }
}

#[test]
fn event_store_roundtrips_payloads_in_order() {
    let path = temp_path("roundtrip");
    let store = TraceEventStore::new(&path);
    assert!(
        store.load().expect("missing file is empty").is_empty(),
        "缺文件应为空"
    );

    let step = replay_step();
    let first = TraceEvent::new("t1", 1, TraceEventKind::PlanPatched, "plan", 100).with_payload(
        TracePayload::Plan {
            revision: step.revision,
            tasks: step.tasks.clone(),
        },
    );
    let second = TraceEvent::new("t1", 2, TraceEventKind::LimitsCaptured, "limits", 100)
        .with_payload(TracePayload::Limits {
            limits: step.limits.clone(),
        });
    store.append(&first).unwrap();
    store.append(&second).unwrap();
    assert_eq!(store.load().unwrap(), vec![first, second]);
}

#[test]
fn event_store_reports_bad_line_numbers_and_accepts_crlf() {
    let path = temp_path("bad-line");
    let event = TraceEvent::new("t1", 1, TraceEventKind::TaskSelected, "task", 100);
    let line = serde_json::to_string(&event).unwrap();
    std::fs::write(&path, format!("{line}\r\nnot-json\r\n")).unwrap();

    let store = TraceEventStore::new(&path);
    assert!(matches!(
        store.load(),
        Err(Error::TraceParse { line: 2, .. })
    ));

    std::fs::write(&path, format!("{line}\r\n")).unwrap();
    assert_eq!(store.load().unwrap(), vec![event], "CRLF 不能被当成坏行");
}

#[test]
fn event_store_rejects_schema_drift_on_append_and_load() {
    let path = temp_path("schema");
    let store = TraceEventStore::new(&path);
    let mut drifted = TraceEvent::new("t1", 1, TraceEventKind::TaskSelected, "task", 100);
    drifted.schema = 999;
    assert!(matches!(
        store.append(&drifted),
        Err(Error::TraceSchema {
            found: 999,
            expected: 1
        })
    ));
    assert!(store.load().unwrap().is_empty(), "被拒事件不能落盘");

    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&drifted).unwrap()),
    )
    .unwrap();
    assert!(matches!(
        store.load(),
        Err(Error::TraceSchema {
            found: 999,
            expected: 1
        })
    ));
}

#[test]
fn legacy_step_migration_produces_four_events_and_folds_back() {
    let step = replay_step();
    let events = events_from_step("t1", 1, &step, 100);
    assert_eq!(events.len(), 4);
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );

    let mut log = TraceLog::new("t1");
    for event in events {
        log.append(event).unwrap();
    }
    assert_eq!(log.latest_step().unwrap(), step);
}

#[test]
fn event_store_migrates_legacy_steps_into_a_replayable_log() {
    let path = temp_path("migrate");
    let store = TraceEventStore::new(&path);
    let step = replay_step();
    store.append_step("t1", 1, &step, 100).unwrap();

    let log = store.load_log("t1").unwrap();
    let folded = log.latest_step().unwrap();
    assert_eq!(folded, step);

    let plan = Plan::new(folded.revision, folded.tasks.clone()).unwrap();
    let selection = plan
        .select_with_effects(&folded.snapshot, &folded.limits, &folded.ledger)
        .unwrap();
    assert_eq!(selection.selected, ["a"]);
}
