use std::collections::BTreeMap;

use trit_orch::{
    CheckReport, CheckResult, Error, Observation, Phase, Plan, Progress, Resources, Snapshot, Task,
    TaskSpec, Ternary, TokenUsage,
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

fn snapshot(tasks: &[TaskSpec]) -> Snapshot {
    Snapshot {
        revision: 1,
        observations: tasks
            .iter()
            .map(|spec| {
                (
                    spec.task.id.clone(),
                    Observation {
                        authorization: Ternary::True,
                        freshness: Ternary::True,
                        ..Observation::default()
                    },
                )
            })
            .collect(),
    }
}

fn passed() -> CheckReport {
    CheckReport {
        epoch: 0,
        checks: 1,
        result: CheckResult::Passed,
    }
}

#[test]
fn rounds_record_exact_counts_and_totals() {
    let tasks = vec![task("a"), task("b")];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let mut progress = Progress::new();
    assert_eq!(progress.brief(), "rounds=0 dispatched=0");
    let first = plan
        .select(
            &snapshot(&tasks),
            &trit_orch::Limits {
                workers: 1,
                capacity: Resources::new(4, 4),
                tokens: 1000,
            },
        )
        .unwrap();
    let report = progress.record(&first);
    assert_eq!(report.selected, 1);
    assert_eq!(report.held, 1);
    assert_eq!(progress.dispatched(), 1);
    assert_eq!(progress.rounds().len(), 1);
    assert!(
        progress
            .brief()
            .starts_with("rounds=1 dispatched=1 last=rev1 selected=1")
    );
    progress.record(&first);
    assert_eq!(progress.dispatched(), 2, "ORCH_PROGRESS_ROUNDS");
}

#[test]
fn finish_requires_every_task_to_be_decided() {
    let tasks = vec![task("a"), task("b")];
    let mut state = snapshot(&tasks);
    state.observations.get_mut("a").unwrap().phase = Phase::Settled;
    state.observations.get_mut("a").unwrap().evidence = Some(passed());
    assert_eq!(
        Progress::new().finish(&tasks, &state),
        Err(Error::Incomplete {
            pending: 1,
            in_flight: 0,
            inconclusive: 0,
            conflicts: 0
        }),
        "ORCH_FINISH_PENDING"
    );
    state.observations.get_mut("b").unwrap().phase = Phase::InFlight;
    assert_eq!(
        Progress::new().finish(&tasks, &state),
        Err(Error::Incomplete {
            pending: 0,
            in_flight: 1,
            inconclusive: 0,
            conflicts: 0
        })
    );
}

#[test]
fn settled_without_evidence_cannot_be_reported_complete() {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    state.observations.get_mut("a").unwrap().phase = Phase::Settled;
    assert_eq!(
        Progress::new().finish(&tasks, &state),
        Err(Error::Incomplete {
            pending: 0,
            in_flight: 0,
            inconclusive: 1,
            conflicts: 0
        }),
        "ORCH_FINISH_INCONCLUSIVE"
    );
    let mut zero = passed();
    zero.checks = 0;
    state.observations.get_mut("a").unwrap().evidence = Some(zero);
    assert!(Progress::new().finish(&tasks, &state).is_err());
}

#[test]
fn stale_definition_or_freshness_cannot_be_reported_complete() {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    a.evidence = Some(passed());
    state.observations.get_mut("a").unwrap().epoch = 1;
    assert_eq!(
        Progress::new().finish(&tasks, &state),
        Err(Error::Incomplete {
            pending: 0,
            in_flight: 0,
            inconclusive: 1,
            conflicts: 0
        }),
        "ORCH_FINISH_STALE"
    );
    state.observations.get_mut("a").unwrap().epoch = 0;
    state.observations.get_mut("a").unwrap().freshness = Ternary::Unknown;
    assert!(
        Progress::new().finish(&tasks, &state).is_err(),
        "ORCH_FINISH_STALE"
    );
}

#[test]
fn finish_reports_confirmed_and_refuted_separately() {
    let tasks = vec![task("a"), task("b")];
    let mut state = snapshot(&tasks);
    state.observations.get_mut("a").unwrap().phase = Phase::Settled;
    state.observations.get_mut("a").unwrap().evidence = Some(passed());
    let b = state.observations.get_mut("b").unwrap();
    b.phase = Phase::Settled;
    b.evidence = Some(CheckReport {
        epoch: 0,
        checks: 1,
        result: CheckResult::Refuted,
    });
    let summary = Progress::new().finish(&tasks, &state).unwrap();
    assert_eq!(
        summary,
        trit_orch::Summary {
            confirmed: 1,
            refuted: 1,
            rounds: 0
        }
    );
}

#[test]
fn missing_observation_counts_as_pending_not_complete() {
    let tasks = vec![task("a")];
    let state = Snapshot {
        revision: 1,
        observations: BTreeMap::new(),
    };
    assert_eq!(
        Progress::new().finish(&tasks, &state),
        Err(Error::Incomplete {
            pending: 1,
            in_flight: 0,
            inconclusive: 0,
            conflicts: 0
        })
    );
}

#[test]
fn conflicted_evidence_is_reported_separately_from_inconclusive() {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    a.evidence = Some(CheckReport {
        epoch: 0,
        checks: 2,
        result: CheckResult::Conflicted,
    });
    assert_eq!(
        Progress::new().finish(&tasks, &state),
        Err(Error::Incomplete {
            pending: 0,
            in_flight: 0,
            inconclusive: 0,
            conflicts: 1
        })
    );
}

#[test]
fn finish_ignores_predictions_and_only_uses_snapshot_facts() {
    let tasks = vec![task("a")];
    let mut state = snapshot(&tasks);
    state.observations.get_mut("a").unwrap().phase = Phase::InFlight;
    state.observations.get_mut("a").unwrap().usage = TokenUsage {
        fresh: 50,
        cached: 50,
        output: 10,
    };
    assert!(Progress::new().finish(&tasks, &state).is_err());
    assert_eq!(Progress::new().dispatched(), 0);
}
