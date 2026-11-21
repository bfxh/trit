use std::collections::BTreeMap;

use trit_orch::{
    Actor, AppendOutcome, Error, Limits, LogicalTime, Observation, Plan, Resources, Snapshot, Task,
    TaskSpec, Ternary, TraceEvent, TraceEventKind, TraceLog, TracePayload,
};

fn event(trace_id: &str, seq: u64, observed_at: u64) -> TraceEvent {
    TraceEvent::new(
        trace_id,
        seq,
        TraceEventKind::TaskSelected,
        "corr-1",
        observed_at,
    )
}

#[test]
fn trace_log_orders_by_seq_and_marks_late_arrivals() {
    let mut log = TraceLog::new("t1");
    assert_eq!(
        log.append(event("t1", 1, 100)).unwrap(),
        AppendOutcome::Recorded
    );
    assert_eq!(
        log.append(event("t1", 3, 300)).unwrap(),
        AppendOutcome::Recorded
    );
    // seq 2 在 seq 3 之后才到：允许写入，但必须标记为迟到，不能插到 3 前面假装没发生。
    assert_eq!(
        log.append(event("t1", 2, 200)).unwrap(),
        AppendOutcome::LateButRecorded
    );

    let seqs: Vec<u64> = log.events().map(|event| event.seq).collect();
    assert_eq!(seqs, [1, 2, 3]);
    assert!(log.gaps().is_empty());
    assert_eq!(log.len(), 3);
}

#[test]
fn trace_log_deduplicates_event_ids_and_rejects_seq_conflicts() {
    let mut log = TraceLog::new("t1");
    let first = event("t1", 1, 100).with_id("e1");
    assert_eq!(log.append(first.clone()).unwrap(), AppendOutcome::Recorded);
    assert_eq!(log.append(first).unwrap(), AppendOutcome::Duplicate);
    assert_eq!(log.len(), 1, "重复事件不能翻倍");

    let conflicting = event("t1", 1, 110).with_id("e2");
    assert!(matches!(
        log.append(conflicting),
        Err(Error::TraceSeqConflict { seq: 1 })
    ));
    assert_eq!(log.len(), 1, "冲突事件不能静默覆盖");
}

#[test]
fn trace_log_reports_gaps_and_rejects_schema_drift() {
    let mut log = TraceLog::new("t1");
    log.append(event("t1", 1, 100)).unwrap();
    log.append(event("t1", 3, 300)).unwrap();
    assert_eq!(log.gaps(), vec![(2, 2)], "缺 seq 2 必须可见");

    let mut drifted = event("t1", 4, 400);
    drifted.schema = 999;
    assert!(matches!(
        log.append(drifted),
        Err(Error::TraceSchema {
            found: 999,
            expected: 1
        })
    ));
    assert_eq!(log.len(), 2, "schema 不匹配的事件不能入日志");
}

#[test]
fn trace_log_rejects_events_from_another_trace() {
    let mut log = TraceLog::new("t1");
    assert!(matches!(
        log.append(event("t2", 1, 100)),
        Err(Error::TraceMismatch { expected, found })
            if expected == "t1" && found == "t2"
    ));
    assert!(log.events().next().is_none());
}

#[test]
fn trace_event_keeps_event_time_and_observation_time_separate() {
    let logical = LogicalTime {
        revision: 7,
        epoch: 2,
        fence: 9,
        round: 3,
        attempt: 1,
    };
    let event = event("t1", 1, 100)
        .with_event_time(40)
        .with_logical(logical)
        .with_actor(Actor::Agent("alice".into()));

    assert_eq!(event.event_time, Some(40), "事实发生在 40");
    assert_eq!(event.observed_at, 100, "编排在 100 才知道");
    assert_eq!(event.logical, logical);
    assert_eq!(event.actor, Actor::Agent("alice".into()));
    assert_eq!(event.schema, TraceEvent::CURRENT_SCHEMA);
}

#[test]
fn trace_event_allows_unknown_event_time() {
    let event = event("t1", 1, 100);
    assert_eq!(event.event_time, None, "不知道事实何时发生就保持 None");
    assert_eq!(event.observed_at, 100);
}

fn replay_inputs() -> (Vec<TaskSpec>, Snapshot, Limits) {
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
    (tasks, snapshot, limits)
}

#[test]
fn trace_fold_produces_a_replayable_step() {
    let (tasks, snapshot, limits) = replay_inputs();
    let mut log = TraceLog::new("t1");
    log.append(
        TraceEvent::new("t1", 1, TraceEventKind::PlanPatched, "plan", 100).with_payload(
            TracePayload::Plan {
                revision: 7,
                tasks: tasks.clone(),
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t1", 2, TraceEventKind::SnapshotCaptured, "snapshot", 100).with_payload(
            TracePayload::Snapshot {
                snapshot: snapshot.clone(),
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t1", 3, TraceEventKind::LimitsCaptured, "limits", 100).with_payload(
            TracePayload::Limits {
                limits: limits.clone(),
            },
        ),
    )
    .unwrap();

    let step = log.latest_step().unwrap();
    assert_eq!(step.revision, 7);
    assert_eq!(step.tasks, tasks);
    assert_eq!(step.snapshot, snapshot);
    assert_eq!(step.limits, limits);
    assert!(step.ledger.is_empty());

    let plan = Plan::new(step.revision, step.tasks.clone()).unwrap();
    let selection = plan
        .select_with_effects(&step.snapshot, &step.limits, &step.ledger)
        .unwrap();
    assert_eq!(selection.selected, ["a"]);
}

#[test]
fn trace_fold_last_state_wins_and_fails_closed_when_inputs_are_missing() {
    let (tasks, snapshot, limits) = replay_inputs();
    let mut log = TraceLog::new("t1");
    log.append(
        TraceEvent::new("t1", 1, TraceEventKind::PlanPatched, "plan", 100).with_payload(
            TracePayload::Plan {
                revision: 6,
                tasks: Vec::new(),
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t1", 2, TraceEventKind::PlanPatched, "plan", 100).with_payload(
            TracePayload::Plan {
                revision: 7,
                tasks: tasks.clone(),
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t1", 3, TraceEventKind::SnapshotCaptured, "snapshot", 100)
            .with_payload(TracePayload::Snapshot { snapshot }),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t1", 4, TraceEventKind::LimitsCaptured, "limits", 100)
            .with_payload(TracePayload::Limits { limits }),
    )
    .unwrap();

    let step = log.latest_step().unwrap();
    assert_eq!(step.revision, 7);
    assert_eq!(step.tasks, tasks, "后到的完整计划必须覆盖旧计划");

    let mut incomplete = TraceLog::new("t1");
    incomplete
        .append(
            TraceEvent::new("t1", 1, TraceEventKind::PlanPatched, "plan", 100).with_payload(
                TracePayload::Plan {
                    revision: 7,
                    tasks: Vec::new(),
                },
            ),
        )
        .unwrap();
    assert!(matches!(
        incomplete.latest_step(),
        Err(Error::TraceIncomplete { .. })
    ));
}

#[test]
fn trace_fold_rejects_plan_snapshot_revision_mismatch() {
    let (tasks, mut snapshot, limits) = replay_inputs();
    snapshot.revision = 8;
    let mut log = TraceLog::new("t1");
    log.append(
        TraceEvent::new("t1", 1, TraceEventKind::PlanPatched, "plan", 100)
            .with_payload(TracePayload::Plan { revision: 7, tasks }),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t1", 2, TraceEventKind::SnapshotCaptured, "snapshot", 100)
            .with_payload(TracePayload::Snapshot { snapshot }),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t1", 3, TraceEventKind::LimitsCaptured, "limits", 100)
            .with_payload(TracePayload::Limits { limits }),
    )
    .unwrap();

    assert!(matches!(
        log.latest_step(),
        Err(Error::RevisionMismatch {
            expected: 7,
            actual: 8
        })
    ));
}
