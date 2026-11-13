use crate::*;

fn spec(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: Vec::new(),
        },
        requirements: Resources::new(1, 1),
        max_tokens: 10,
        epoch: 0,
        ..TaskSpec::default()
    }
}

fn edge(id: &str, required: Ternary) -> Dependency {
    Dependency {
        task: id.into(),
        required,
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
        workers: 16,
        capacity: Resources::new(16, 16),
        tokens: 1_000,
    }
}

#[test]
fn all_dependency_and_admission_truth_tables() {
    use Ternary::{False, True, Unknown};
    let expected = [
        [True, True, True],
        [Unknown, Unknown, Unknown],
        [False, Unknown, True],
    ];
    for (row, required) in Ternary::ALL.into_iter().enumerate() {
        for (column, evidence) in Ternary::ALL.into_iter().enumerate() {
            assert_eq!(dependency_gate(required, evidence), expected[row][column]);
        }
    }
    for dependencies in Ternary::ALL {
        for authorization in Ternary::ALL {
            for resources in Ternary::ALL {
                for freshness in Ternary::ALL {
                    let inputs = [dependencies, authorization, resources, freshness];
                    let expected = if inputs.contains(&False) {
                        False
                    } else if inputs.contains(&Unknown) {
                        Unknown
                    } else {
                        True
                    };
                    assert_eq!(
                        admit(dependencies, authorization, resources, freshness),
                        expected
                    );
                }
            }
        }
    }
}

#[test]
fn evidence_requires_checks_and_matching_revision() {
    let default = Observation::default();
    assert_eq!(default.phase, Phase::Pending);
    assert_eq!(default.authorization, Ternary::Unknown);
    assert_eq!(default.freshness, Ternary::Unknown);
    assert_eq!(default.evidence, None);
    assert_eq!(default.usage.total(), Ok(0));
    for (result, verdict) in [
        (CheckResult::Passed, Ternary::True),
        (CheckResult::Refuted, Ternary::False),
        (CheckResult::Inconclusive, Ternary::Unknown),
    ] {
        let mut report = CheckReport {
            epoch: 7,
            checks: 1,
            result,
        };
        assert_eq!(report.verdict(7), verdict);
        assert_eq!(report.verdict(8), Ternary::Unknown);
        report.checks = 0;
        assert_eq!(report.verdict(7), Ternary::Unknown);
    }
}

#[test]
fn evidence_judgment_keeps_conflict_separate_from_unknown() {
    let report = |result| CheckReport {
        epoch: 7,
        checks: 1,
        result,
    };
    assert_eq!(
        report(CheckResult::Passed).judgment(7),
        EvidenceVerdict::Proved
    );
    assert_eq!(
        report(CheckResult::Refuted).judgment(7),
        EvidenceVerdict::Refuted
    );
    assert_eq!(
        report(CheckResult::Inconclusive).judgment(7),
        EvidenceVerdict::Neither
    );
    let conflicted = report(CheckResult::Conflicted);
    assert_eq!(conflicted.judgment(7), EvidenceVerdict::Conflicted);
    assert_eq!(conflicted.judgment(8), EvidenceVerdict::Neither);
    assert_eq!(conflicted.verdict(7), Ternary::Unknown);
    let zero_checks = CheckReport {
        epoch: 7,
        checks: 0,
        result: CheckResult::Conflicted,
    };
    assert_eq!(zero_checks.judgment(7), EvidenceVerdict::Neither);
}

#[test]
fn every_declared_edge_is_validated() {
    for required in Ternary::ALL {
        let mut task = spec("a");
        task.task.deps = vec![edge("missing", required)];
        assert!(matches!(
            Plan::new(7, vec![task.clone()]),
            Err(Error::UnknownDependency { .. })
        ));
        task.task.deps = vec![edge("a", required)];
        assert!(matches!(
            Plan::new(7, vec![task.clone()]),
            Err(Error::Cycle { .. })
        ));
        task.task.deps = vec![edge("b", required), edge("b", Ternary::True)];
        assert!(matches!(
            Plan::new(7, vec![task, spec("b")]),
            Err(Error::DuplicateDependency { .. })
        ));
    }
    assert!(matches!(
        Plan::new(7, vec![spec(" \t\n")]),
        Err(Error::BlankTaskId { .. })
    ));
    assert!(matches!(
        Plan::new(7, vec![spec("a"), spec("a")]),
        Err(Error::DuplicateTask { .. })
    ));
    let mut a = spec("a");
    let mut b = spec("b");
    a.task.deps.push(edge("b", Ternary::False));
    b.task.deps.push(edge("a", Ternary::Unknown));
    assert!(matches!(Plan::new(7, vec![a, b]), Err(Error::Cycle { .. })));
}

#[test]
fn long_chain_uses_iterative_validation_and_canonical_order() {
    let mut tasks = Vec::new();
    let mut previous: Option<String> = None;
    for index in 0..10_000 {
        let id = format!("t{index:05}");
        let mut task = spec(&id);
        if let Some(ref predecessor) = previous {
            task.task.deps.push(edge(predecessor, Ternary::True));
        }
        tasks.push(task);
        previous = Some(id);
    }
    let plan = Plan::new(7, tasks.clone()).unwrap();
    tasks.reverse();
    assert_eq!(plan, Plan::new(7, tasks.clone()).unwrap());
    assert_eq!(plan.revision(), 7);
    assert_eq!(
        plan.select(&snapshot(&tasks), &limits()).unwrap().selected,
        ["t00000"]
    );
}

#[test]
fn snapshot_ids_revision_and_workers_are_not_guessed() {
    let tasks = [spec("a"), spec("b")];
    let plan = Plan::new(7, tasks.to_vec()).unwrap();
    let mut state = snapshot(&tasks);
    state.observations.remove("a");
    state
        .observations
        .insert("extra".into(), Observation::default());
    assert_eq!(
        plan.select(&state, &limits()),
        Err(Error::ObservationMismatch {
            missing: vec!["a".into()],
            unexpected: vec!["extra".into()],
        })
    );
    state.revision = 8;
    assert_eq!(
        plan.select(&state, &limits()),
        Err(Error::RevisionMismatch {
            expected: 7,
            actual: 8
        })
    );
    let empty = Plan::new(7, Vec::new()).unwrap();
    let mut limit = limits();
    limit.workers = 0;
    assert_eq!(empty.select(&snapshot(&[]), &limit), Err(Error::NoWorkers));
    assert!(
        empty
            .select(&snapshot(&[]), &limits())
            .unwrap()
            .selected
            .is_empty()
    );
}

#[test]
fn only_settled_current_evidence_can_unlock_successors() {
    let a = spec("a");
    let mut b = spec("b");
    b.task.deps.push(edge("a", Ternary::True));
    let tasks = [a, b];
    let plan = Plan::new(7, tasks.to_vec()).unwrap();
    let mut state = snapshot(&tasks);
    let passed = CheckReport {
        epoch: 0,
        checks: 1,
        result: CheckResult::Passed,
    };
    state.observations.get_mut("a").unwrap().evidence = Some(passed.clone());
    let original = state.clone();
    let batch = plan.select(&state, &limits()).unwrap();
    assert_eq!(state, original);
    assert_eq!(batch.selected, ["a"]);
    assert_eq!(batch.decisions[1].admission, Ternary::Unknown);
    state.observations.get_mut("a").unwrap().phase = Phase::InFlight;
    let batch = plan.select(&state, &limits()).unwrap();
    assert!(batch.selected.is_empty());
    assert_eq!(batch.decisions[0].reasons, [BlockReason::InFlight]);
    assert_eq!(batch.decisions[0].admission, Ternary::Unknown);
    state.observations.get_mut("a").unwrap().phase = Phase::Settled;
    assert_eq!(plan.select(&state, &limits()).unwrap().selected, ["b"]);
    state.observations.get_mut("a").unwrap().evidence = None;
    let batch = plan.select(&state, &limits()).unwrap();
    assert!(batch.selected.is_empty());
    assert_eq!(batch.decisions[0].admission, Ternary::Unknown);
    assert_eq!(batch.decisions[0].reasons, [BlockReason::Settled]);
    for (epoch, checks, result, freshness, expected) in [
        (6, 1, CheckResult::Passed, Ternary::True, Ternary::Unknown),
        (0, 0, CheckResult::Passed, Ternary::True, Ternary::Unknown),
        (
            0,
            1,
            CheckResult::Inconclusive,
            Ternary::True,
            Ternary::Unknown,
        ),
        (0, 1, CheckResult::Refuted, Ternary::True, Ternary::False),
        (
            0,
            1,
            CheckResult::Passed,
            Ternary::Unknown,
            Ternary::Unknown,
        ),
        (0, 1, CheckResult::Passed, Ternary::False, Ternary::Unknown),
    ] {
        let predecessor = state.observations.get_mut("a").unwrap();
        predecessor.evidence = Some(CheckReport {
            epoch,
            checks,
            result,
        });
        predecessor.freshness = freshness;
        let batch = plan.select(&state, &limits()).unwrap();
        assert!(batch.selected.is_empty());
        assert_eq!(batch.decisions[1].admission, expected);
    }
}

#[test]
fn full_ordering_skips_tasks_that_cannot_fit() {
    let mut heavy = spec("heavy");
    heavy.task.priority = 9;
    heavy.requirements = Resources::new(3, 3);
    let mut impossible = spec("impossible");
    impossible.task.priority = 10;
    impossible.max_tokens = 1_001;
    let mut long = spec("long");
    long.task.duration = 2;
    let tasks = vec![
        spec("a"),
        spec("z"),
        heavy,
        impossible,
        long,
        spec("running"),
    ];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut state = snapshot(&tasks);
    state.observations.get_mut("running").unwrap().phase = Phase::InFlight;
    let limit = Limits {
        workers: 3,
        capacity: Resources::new(3, 3),
        ..limits()
    };
    let batch = plan.select(&state, &limit).unwrap();
    assert_eq!(batch.selected, ["long", "z"]);
    assert_eq!(batch.occupied_workers, 3);
    assert_eq!(batch.reserved_resources, Resources::new(3, 3));
    assert_eq!(
        batch
            .decisions
            .iter()
            .map(|d| d.task.as_str())
            .collect::<Vec<_>>(),
        ["a", "heavy", "impossible", "long", "running", "z"]
    );
    assert_eq!(batch.decisions[1].admission, Ternary::Unknown);
    assert_eq!(batch.decisions[2].admission, Ternary::False);
    let mut shuffled = tasks;
    shuffled.reverse();
    assert_eq!(
        batch,
        Plan::new(7, shuffled)
            .unwrap()
            .select(&state, &limit)
            .unwrap()
    );
}

#[test]
fn observed_and_reserved_tokens_are_separate_and_complete() {
    let mut running = spec("running");
    running.max_tokens = 100;
    let mut settled = spec("settled");
    settled.max_tokens = 40;
    let mut pending = spec("pending");
    pending.max_tokens = 80;
    let tasks = [running, settled, pending];
    let mut state = snapshot(&tasks);
    let run = state.observations.get_mut("running").unwrap();
    run.phase = Phase::InFlight;
    run.usage = TokenUsage {
        fresh: 10,
        cached: 20,
        output: 30,
    };
    let done = state.observations.get_mut("settled").unwrap();
    done.phase = Phase::Settled;
    done.usage = TokenUsage {
        fresh: 5,
        cached: 10,
        output: 5,
    };
    state.observations.get_mut("pending").unwrap().usage = TokenUsage {
        fresh: 5,
        cached: 5,
        output: 0,
    };
    let plan = Plan::new(7, tasks.to_vec()).unwrap();
    let mut limit = Limits {
        tokens: 190,
        ..limits()
    };
    let batch = plan.select(&state, &limit).unwrap();
    assert!(batch.selected.is_empty());
    assert_eq!(batch.reserved_tokens, 40);
    assert_eq!(
        batch.decisions[0].reasons,
        [BlockReason::Tokens {
            required: 70,
            available: 60
        }],
        "已花掉的 10 计入 observed，剩余额度只按 70 申请"
    );
    limit.tokens = 200;
    let batch = plan.select(&state, &limit).unwrap();
    assert_eq!(batch.selected, ["pending"]);
    assert_eq!(
        batch.observed_tokens,
        TokenUsage {
            fresh: 20,
            cached: 35,
            output: 35
        }
    );
    assert_eq!(batch.reserved_tokens, 110);
    assert_eq!(batch.reserved_resources, Resources::new(2, 2));
    assert_eq!(batch.occupied_workers, 2);
}

#[test]
fn snapshot_overcommit_and_arithmetic_overflow_are_errors() {
    let tasks = [spec("a"), spec("b")];
    let plan = Plan::new(7, tasks.to_vec()).unwrap();
    let mut state = snapshot(&tasks);
    for observation in state.observations.values_mut() {
        observation.phase = Phase::InFlight;
    }
    assert!(matches!(
        plan.select(
            &state,
            &Limits {
                workers: 1,
                ..limits()
            }
        ),
        Err(Error::WorkerLimitExceeded { .. })
    ));
    assert!(matches!(
        plan.select(
            &state,
            &Limits {
                capacity: Resources::new(1, 2),
                ..limits()
            }
        ),
        Err(Error::ResourceLimitExceeded { .. })
    ));
    assert!(matches!(
        plan.select(
            &state,
            &Limits {
                tokens: 19,
                ..limits()
            }
        ),
        Err(Error::TokenLimitExceeded { .. })
    ));
    state.observations.get_mut("a").unwrap().usage.fresh = 11;
    assert_eq!(
        plan.select(&state, &limits()),
        Err(Error::TaskTokenLimitExceeded {
            task: "a".into(),
            used: 11,
            max_tokens: 10,
        })
    );
    assert!(matches!(
        (TokenUsage {
            fresh: u64::MAX,
            cached: 1,
            output: 0
        })
        .total(),
        Err(Error::TokenOverflow { .. })
    ));
    assert!(matches!(
        (TokenUsage {
            fresh: u64::MAX,
            cached: 0,
            output: 1
        })
        .total(),
        Err(Error::TokenOverflow { .. })
    ));
    let mut enormous = tasks;
    for task in &mut enormous {
        task.max_tokens = u64::MAX;
    }
    state.observations.get_mut("a").unwrap().usage.fresh = 0;
    let plan = Plan::new(7, enormous.to_vec()).unwrap();
    assert!(matches!(
        plan.select(
            &state,
            &Limits {
                tokens: u64::MAX,
                ..limits()
            }
        ),
        Err(Error::TokenOverflow { .. })
    ));
    enormous[0].requirements.cpu = u32::MAX;
    assert!(matches!(
        Plan::new(7, enormous.to_vec())
            .unwrap()
            .select(&state, &limits()),
        Err(Error::ResourceOverflow { .. })
    ));
}
