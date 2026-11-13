use std::collections::BTreeMap;

use trit_orch::{
    Error, Ledger, Limits, Observation, Plan, ReplayDivergenceKind, Resources, Snapshot, Task,
    TaskSpec, Ternary, TraceEvent, TraceEventKind, TraceLog, TracePayload, TraceRecorder,
};

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
    (tasks, snapshot, limits)
}

fn seeded_log(tasks: &[TaskSpec], snapshot: &Snapshot, limits: &Limits) -> TraceLog {
    let mut log = TraceLog::new("t1");
    log.append(
        TraceEvent::new("t1", 1, TraceEventKind::PlanPatched, "plan", 100).with_payload(
            TracePayload::Plan {
                revision: 7,
                tasks: tasks.to_vec(),
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
    log
}

#[test]
fn state_at_folds_only_events_up_to_the_requested_seq() {
    let (tasks, snapshot, limits) = replay_inputs();
    let mut log = seeded_log(&tasks, &snapshot, &limits);

    let mut later = limits.clone();
    later.workers = 99;
    log.append(
        TraceEvent::new("t1", 4, TraceEventKind::LimitsCaptured, "limits", 100).with_payload(
            TracePayload::Limits {
                limits: later.clone(),
            },
        ),
    )
    .unwrap();

    let at3 = log.step_at(3).unwrap();
    assert_eq!(at3.limits, limits, "seq 3 时还不知道 seq 4 的修改");
    let at4 = log.step_at(4).unwrap();
    assert_eq!(at4.limits, later);
    assert_eq!(log.latest_step().unwrap(), at4);
    assert_eq!(log.state_at(3).limits, Some(limits));
}

#[test]
fn why_walks_causation_and_reports_missing_causes_and_cycles() {
    let mut log = TraceLog::new("t1");
    let e1 = TraceEvent::new("t1", 1, TraceEventKind::TaskSelected, "c", 100).with_id("e1");
    let e2 = TraceEvent::new("t1", 2, TraceEventKind::EffectPrepared, "c", 101)
        .with_id("e2")
        .with_causation_id("e1");
    let e3 = TraceEvent::new("t1", 3, TraceEventKind::EffectSending, "c", 102)
        .with_id("e3")
        .with_causation_id("e2");
    for event in [e1, e2, e3] {
        log.append(event).unwrap();
    }

    let chain = log.why("e3").unwrap();
    assert_eq!(
        chain
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["e1", "e2", "e3"]
    );
    assert!(chain.missing.is_empty());

    let orphan = TraceEvent::new("t1", 4, TraceEventKind::EffectUnknown, "c", 103)
        .with_id("e4")
        .with_causation_id("ghost");
    log.append(orphan).unwrap();
    let chain = log.why("e4").unwrap();
    assert_eq!(
        chain
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["e4"]
    );
    assert_eq!(chain.missing, vec!["ghost".to_owned()]);
    assert!(matches!(
        log.why("nope"),
        Err(Error::TraceEventMissing { .. })
    ));

    let mut cycle = TraceLog::new("t1");
    let a = TraceEvent::new("t1", 1, TraceEventKind::TaskSelected, "c", 100)
        .with_id("a")
        .with_causation_id("b");
    let b = TraceEvent::new("t1", 2, TraceEventKind::TaskSelected, "c", 101)
        .with_id("b")
        .with_causation_id("a");
    cycle.append(a).unwrap();
    cycle.append(b).unwrap();
    assert!(matches!(
        cycle.why("a"),
        Err(Error::TraceCausalityCycle { .. })
    ));
}

#[test]
fn replay_report_confirms_matching_selection_and_flags_mismatch_and_missing_inputs() {
    let (tasks, snapshot, limits) = replay_inputs();
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let selection = plan
        .select_with_effects(&snapshot, &limits, &Ledger::new())
        .unwrap();

    let mut good = seeded_log(&tasks, &snapshot, &limits);
    let mut recorder = TraceRecorder::new("t1", 4);
    good.append(recorder.selection(&selection, 100)).unwrap();
    let report = good.replay_selections().unwrap();
    assert_eq!(report.steps, 1);
    assert_eq!(report.agreed, 1);
    assert!(report.divergences.is_empty());

    let mut bad = seeded_log(&tasks, &snapshot, &limits);
    bad.append(
        TraceEvent::new("t1", 4, TraceEventKind::SelectionCaptured, "selection", 100).with_payload(
            TracePayload::Selection {
                revision: 7,
                selected: vec!["wrong".into()],
            },
        ),
    )
    .unwrap();
    let report = bad.replay_selections().unwrap();
    assert_eq!(report.steps, 1);
    assert_eq!(report.agreed, 0);
    assert_eq!(report.divergences.len(), 1);
    assert_eq!(
        report.divergences[0].kind,
        ReplayDivergenceKind::KernelChanged
    );
    assert_eq!(report.divergences[0].recorded, ["wrong"]);
    assert_eq!(report.divergences[0].replayed, ["a"]);

    let mut missing = TraceLog::new("t1");
    missing
        .append(
            TraceEvent::new("t1", 1, TraceEventKind::SelectionCaptured, "selection", 100)
                .with_payload(TracePayload::Selection {
                    revision: 7,
                    selected: vec!["a".into()],
                }),
        )
        .unwrap();
    let report = missing.replay_selections().unwrap();
    assert_eq!(
        report.divergences[0].kind,
        ReplayDivergenceKind::MissingEvent
    );
}
