use trit_orch::{
    CheckReport, CheckResult, Dependency, Error, EvidenceRails, EvidenceVerdict, Limits,
    ObligationKind, Observation, Phase, Plan, Progress, Resources, Snapshot, Task, TaskSpec,
    Ternary,
};

fn task(id: &str, deps: Vec<Dependency>) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps,
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
        ..TaskSpec::default()
    }
}

fn report(result: CheckResult) -> CheckReport {
    CheckReport {
        epoch: 0,
        checks: 1,
        result,
    }
}

fn snapshot(tasks: &[TaskSpec]) -> Snapshot {
    Snapshot {
        revision: 7,
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

fn limits() -> Limits {
    Limits {
        workers: 4,
        capacity: Resources::new(100, 100),
        tokens: 10_000,
    }
}

#[test]
fn rails_derive_belnap_four_values() {
    let mut rails = EvidenceRails::default();
    assert_eq!(rails.judgment(0), EvidenceVerdict::Neither);

    rails.push("alice", report(CheckResult::Passed));
    assert_eq!(rails.judgment(0), EvidenceVerdict::Proved);

    rails.push("bob", report(CheckResult::Refuted));
    assert_eq!(rails.judgment(0), EvidenceVerdict::Conflicted);
    assert_eq!(rails.support.len(), 1);
    assert_eq!(rails.counter.len(), 1);

    let mut conflict_only = EvidenceRails::default();
    conflict_only.push("carol", report(CheckResult::Conflicted));
    assert_eq!(conflict_only.judgment(0), EvidenceVerdict::Conflicted);
    assert_eq!(conflict_only.support.len(), 1);
    assert_eq!(conflict_only.counter.len(), 1);

    let mut stale = EvidenceRails::default();
    stale.push(
        "stale",
        CheckReport {
            epoch: 6,
            checks: 1,
            result: CheckResult::Passed,
        },
    );
    stale.push(
        "zero",
        CheckReport {
            epoch: 0,
            checks: 0,
            result: CheckResult::Refuted,
        },
    );
    assert_eq!(stale.judgment(0), EvidenceVerdict::Neither);

    let legacy = Observation {
        evidence: Some(report(CheckResult::Passed)),
        freshness: Ternary::True,
        ..Observation::default()
    };
    assert_eq!(legacy.judgment(0), EvidenceVerdict::Proved);
}

#[test]
fn selection_routes_multi_source_conflict_to_adjudicate() {
    let tasks = vec![
        task("dep", Vec::new()),
        task(
            "blocked",
            vec![Dependency {
                task: "dep".into(),
                required: Ternary::True,
            }],
        ),
    ];
    let mut state = snapshot(&tasks);
    let mut rails = EvidenceRails::default();
    rails.push("alice", report(CheckResult::Passed));
    rails.push("bob", report(CheckResult::Refuted));
    let dep = state.observations.get_mut("dep").unwrap();
    dep.phase = Phase::Settled;
    dep.evidence_rails = rails;

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select(&state, &limits())
        .unwrap();
    let blocked = selection
        .decisions
        .iter()
        .find(|decision| decision.task == "blocked")
        .unwrap();
    assert_eq!(blocked.admission, Ternary::Unknown);
    assert!(
        blocked
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Adjudicate)
    );
}

#[test]
fn progress_counts_multi_source_conflict() {
    let tasks = vec![task("a", Vec::new())];
    let mut state = snapshot(&tasks);
    let mut rails = EvidenceRails::default();
    rails.push("alice", report(CheckResult::Passed));
    rails.push("bob", report(CheckResult::Refuted));
    let a = state.observations.get_mut("a").unwrap();
    a.phase = Phase::Settled;
    a.evidence_rails = rails;

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
