use trit_orch::{
    CheckReport, CheckResult, Observation, Phase, Resources, Snapshot, Task, TaskSpec, Ternary,
    undecided_mass,
};

fn task(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: Vec::new(),
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
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

fn set_evidence(state: &mut Snapshot, id: &str, result: CheckResult) {
    let observation = state.observations.get_mut(id).unwrap();
    observation.phase = Phase::Settled;
    observation.evidence = Some(CheckReport {
        epoch: 0,
        checks: 1,
        result,
    });
    observation.freshness = Ternary::True;
}

#[test]
fn mass_counts_unknowns_and_conflicts() {
    let tasks = vec![
        task("proved"),
        task("unknown"),
        task("conflict"),
        task("pending"),
        task("stale"),
    ];
    let mut state = snapshot(&tasks);
    set_evidence(&mut state, "proved", CheckResult::Passed);
    set_evidence(&mut state, "unknown", CheckResult::Inconclusive);
    set_evidence(&mut state, "conflict", CheckResult::Conflicted);
    let stale = state.observations.get_mut("stale").unwrap();
    stale.phase = Phase::Settled;
    stale.evidence = Some(CheckReport {
        epoch: 0,
        checks: 1,
        result: CheckResult::Passed,
    });
    stale.freshness = Ternary::False;

    let mass = undecided_mass(&tasks, &state);
    assert_eq!(mass.unknown, 3, "inconclusive + pending + stale");
    assert_eq!(mass.conflicts, 1);
    assert_eq!(mass.mass, 5, "3 个 U + 1 个冲突按 2 计");
}

#[test]
fn mass_decreases_as_unknowns_resolve() {
    let tasks = vec![task("a"), task("b")];
    let mut state = snapshot(&tasks);

    let start = undecided_mass(&tasks, &state);
    assert_eq!(start.mass, 2);
    assert!(!start.is_zero());

    set_evidence(&mut state, "a", CheckResult::Passed);
    let after_a = undecided_mass(&tasks, &state);
    assert_eq!(after_a.mass, 1);
    assert!(after_a.decreased_from(&start));

    set_evidence(&mut state, "b", CheckResult::Refuted);
    let done = undecided_mass(&tasks, &state);
    assert_eq!(done.mass, 0);
    assert!(done.is_zero());
    assert!(done.decreased_from(&after_a));
}
