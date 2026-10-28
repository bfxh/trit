use std::collections::BTreeMap;

use trit_orch::{
    CheckReport, CheckResult, Dependency, Limits, Observation, Phase, Plan, Resources, Snapshot,
    Task, TaskSpec, Ternary, TokenUsage,
};

fn task(id: &str, priority: i32, deps: Vec<Dependency>) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority,
            duration: 1,
            deps,
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
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
            .map(|t| {
                (
                    t.task.id.clone(),
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

fn limits(workers: usize) -> Limits {
    Limits {
        workers,
        capacity: Resources::new(100, 100),
        tokens: 10_000,
    }
}

fn passed() -> CheckReport {
    CheckReport {
        revision: 7,
        checks: 1,
        result: CheckResult::Passed,
    }
}

#[test]
fn selected_predecessor_is_not_imagined_complete() {
    let tasks = vec![
        task("a", 1, vec![]),
        task("b", 2, vec![edge("a", Ternary::True)]),
    ];
    let mut state = snapshot(&tasks);
    state.observations.get_mut("a").unwrap().evidence = Some(passed());
    let plan = Plan::new(7, tasks).unwrap();
    let original = state.clone();
    let result = plan.select(&state, &limits(2)).unwrap();
    assert_eq!(result.selected, vec!["a"], "ORCH_NO_FORECAST_COMPLETION");
    assert_eq!(state, original, "selection must not mutate its input");
    assert_eq!(
        result
            .decisions
            .iter()
            .find(|d| d.task == "b")
            .unwrap()
            .admission,
        Ternary::Unknown
    );
    state.observations.get_mut("a").unwrap().phase = Phase::Settled;
    assert_eq!(plan.select(&state, &limits(2)).unwrap().selected, vec!["b"]);
}

#[test]
fn unknown_and_optional_requirements_remain_distinct() {
    let tasks = vec![
        task("a", 0, vec![]),
        task("unknown", 2, vec![edge("a", Ternary::Unknown)]),
        task("optional", 1, vec![edge("a", Ternary::False)]),
    ];
    let mut state = snapshot(&tasks);
    let source = state.observations.get_mut("a").unwrap();
    source.phase = Phase::Settled;
    source.evidence = Some(passed());
    let plan = Plan::new(7, tasks).unwrap();
    let result = plan.select(&state, &limits(3)).unwrap();
    assert_eq!(result.selected, vec!["optional"]);
    let blocked = result
        .decisions
        .iter()
        .find(|d| d.task == "unknown")
        .unwrap();
    assert_eq!(blocked.admission, Ternary::Unknown);
    assert!(!blocked.reasons.is_empty());
}

#[test]
fn unverified_refuted_and_stale_results_do_not_unlock_dependents() {
    let tasks = vec![
        task("a", 0, vec![]),
        task("b", 1, vec![edge("a", Ternary::True)]),
    ];
    let mut state = snapshot(&tasks);
    state.observations.get_mut("a").unwrap().phase = Phase::Settled;
    let plan = Plan::new(7, tasks).unwrap();
    for (report, expected) in [
        (None, Ternary::Unknown),
        (
            Some(CheckReport {
                revision: 6,
                ..passed()
            }),
            Ternary::Unknown,
        ),
        (
            Some(CheckReport {
                checks: 0,
                ..passed()
            }),
            Ternary::Unknown,
        ),
        (
            Some(CheckReport {
                result: CheckResult::Refuted,
                ..passed()
            }),
            Ternary::False,
        ),
        (
            Some(CheckReport {
                result: CheckResult::Inconclusive,
                ..passed()
            }),
            Ternary::Unknown,
        ),
    ] {
        state.observations.get_mut("a").unwrap().evidence = report;
        let result = plan.select(&state, &limits(2)).unwrap();
        assert_eq!(result.selected, Vec::<String>::new());
        assert_eq!(
            result
                .decisions
                .iter()
                .find(|d| d.task == "b")
                .unwrap()
                .admission,
            expected
        );
    }
    let source = state.observations.get_mut("a").unwrap();
    source.evidence = Some(passed());
    source.freshness = Ternary::Unknown;
    assert_eq!(
        plan.select(&state, &limits(2)).unwrap().selected,
        Vec::<String>::new()
    );
}

#[test]
fn revocation_and_freshness_are_rechecked_each_selection() {
    let tasks = vec![task("a", 1, vec![])];
    let mut state = snapshot(&tasks);
    let plan = Plan::new(7, tasks).unwrap();
    assert_eq!(plan.select(&state, &limits(1)).unwrap().selected, vec!["a"]);
    for value in [Ternary::Unknown, Ternary::False] {
        state.observations.get_mut("a").unwrap().authorization = value;
        let result = plan.select(&state, &limits(1)).unwrap();
        assert_eq!(result.selected.len(), 0, "ORCH_AUTHORIZATION_RECHECK");
        assert_eq!(result.decisions[0].admission, value);
    }
    state.observations.get_mut("a").unwrap().authorization = Ternary::True;
    state.observations.get_mut("a").unwrap().freshness = Ternary::Unknown;
    assert_eq!(plan.select(&state, &limits(1)).unwrap().selected.len(), 0);
}

#[test]
fn selection_is_permutation_invariant_and_preserves_priority_duration_id_order() {
    let mut tasks = vec![
        task("a", 2, vec![]),
        task("z", 2, vec![]),
        task("b", 1, vec![]),
    ];
    tasks[2].task.duration = 99;
    let state = snapshot(&tasks);
    let expected = Plan::new(7, tasks.clone())
        .unwrap()
        .select(&state, &limits(3))
        .unwrap();
    assert_eq!(expected.selected, vec!["z", "a", "b"]);
    for _ in 0..tasks.len() {
        tasks.rotate_left(1);
        assert_eq!(
            Plan::new(7, tasks.clone())
                .unwrap()
                .select(&state, &limits(3))
                .unwrap(),
            expected
        );
    }
    tasks.reverse();
    assert_eq!(
        Plan::new(7, tasks)
            .unwrap()
            .select(&state, &limits(3))
            .unwrap(),
        expected
    );
    assert_eq!(
        expected
            .decisions
            .iter()
            .map(|d| d.task.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "z"]
    );
}

#[test]
fn inflight_work_keeps_worker_and_resource_reservations() {
    let mut tasks = vec![
        task("running", 0, vec![]),
        task("large", 10, vec![]),
        task("small", 1, vec![]),
    ];
    tasks[0].requirements = Resources::new(2, 2);
    tasks[1].requirements = Resources::new(3, 3);
    let mut state = snapshot(&tasks);
    let running = state.observations.get_mut("running").unwrap();
    running.phase = Phase::InFlight;
    running.evidence = Some(passed());
    let plan = Plan::new(7, tasks).unwrap();
    let result = plan
        .select(
            &state,
            &Limits {
                capacity: Resources::new(4, 4),
                ..limits(3)
            },
        )
        .unwrap();
    assert_eq!(result.selected, vec!["small"], "ORCH_INFLIGHT_CAPACITY");
    assert_eq!(result.reserved_resources, Resources::new(3, 3));
    assert_eq!(result.occupied_workers, 2);
    assert_eq!(plan.select(&state, &limits(1)).unwrap().selected.len(), 0);
}

#[test]
fn token_reservations_include_cached_usage_without_double_counting_inflight_spend() {
    let tasks = vec![
        task("running", 0, vec![]),
        task("settled", 0, vec![]),
        task("new", 1, vec![]),
    ];
    let mut state = snapshot(&tasks);
    let running = state.observations.get_mut("running").unwrap();
    running.phase = Phase::InFlight;
    running.usage = TokenUsage {
        fresh: 20,
        cached: 10,
        output: 10,
    };
    let settled = state.observations.get_mut("settled").unwrap();
    settled.phase = Phase::Settled;
    settled.usage.output = 50;
    let plan = Plan::new(7, tasks).unwrap();
    let result = plan
        .select(
            &state,
            &Limits {
                tokens: 250,
                ..limits(2)
            },
        )
        .unwrap();
    assert_eq!(result.selected, vec!["new"], "ORCH_TOKEN_RESERVATION");
    assert_eq!(
        result.observed_tokens,
        TokenUsage {
            fresh: 20,
            cached: 10,
            output: 60
        }
    );
    assert_eq!(result.reserved_tokens, 160);
    assert_eq!(
        result.observed_tokens.total().unwrap() + result.reserved_tokens,
        250
    );
    let below = plan
        .select(
            &state,
            &Limits {
                tokens: 249,
                ..limits(2)
            },
        )
        .unwrap();
    assert_eq!(below.selected.len(), 0, "ORCH_TOKEN_LIMIT");
}

#[test]
fn new_batch_cannot_overbook_workers_or_tokens() {
    let tasks = vec![
        task("a", 3, vec![]),
        task("b", 2, vec![]),
        task("c", 1, vec![]),
    ];
    let state = snapshot(&tasks);
    let plan = Plan::new(7, tasks).unwrap();
    assert_eq!(plan.select(&state, &limits(1)).unwrap().selected, vec!["a"]);
    let result = plan
        .select(
            &state,
            &Limits {
                tokens: 150,
                ..limits(3)
            },
        )
        .unwrap();
    assert_eq!(result.selected, vec!["a"], "ORCH_BATCH_TOKEN_BOUND");
    assert_eq!(result.reserved_tokens, 100);
}

#[test]
fn impossible_task_is_rejected_but_does_not_starve_a_smaller_task() {
    let mut tasks = vec![task("large", 2, vec![]), task("small", 1, vec![])];
    tasks[0].requirements = Resources::new(101, 1);
    let state = snapshot(&tasks);
    let plan = Plan::new(7, tasks).unwrap();
    let result = plan.select(&state, &limits(2)).unwrap();
    assert_eq!(result.selected, vec!["small"]);
    assert_eq!(
        result
            .decisions
            .iter()
            .find(|d| d.task == "large")
            .unwrap()
            .admission,
        Ternary::False
    );
}

#[test]
fn snapshot_requires_matching_revision_and_exact_task_set() {
    let tasks = vec![task("a", 0, vec![])];
    let state = snapshot(&tasks);
    let plan = Plan::new(7, tasks).unwrap();
    assert_eq!(plan.revision(), 7);
    let mut stale = state.clone();
    stale.revision = 6;
    assert!(
        plan.select(&stale, &limits(1)).is_err(),
        "ORCH_STALE_SNAPSHOT"
    );
    let mut missing = state.clone();
    missing.observations.clear();
    assert!(plan.select(&missing, &limits(1)).is_err());
    let mut extra = state;
    extra
        .observations
        .insert("ghost".into(), Observation::default());
    assert!(plan.select(&extra, &limits(1)).is_err());
}

#[test]
fn invalid_graphs_are_rejected_including_inactive_edges() {
    assert!(Plan::new(7, vec![task(" ", 0, vec![])]).is_err());
    assert!(Plan::new(7, vec![task("a", 0, vec![]), task("a", 1, vec![])]).is_err());
    for requirement in [Ternary::False, Ternary::Unknown, Ternary::True] {
        assert!(Plan::new(7, vec![task("a", 0, vec![edge("missing", requirement)])]).is_err());
        assert!(Plan::new(7, vec![task("a", 0, vec![edge("a", requirement)])]).is_err());
        assert!(
            Plan::new(
                7,
                vec![
                    task("a", 0, vec![edge("b", requirement)]),
                    task("b", 1, vec![edge("a", requirement)]),
                ]
            )
            .is_err()
        );
    }
    assert!(
        Plan::new(
            7,
            vec![
                task("a", 0, vec![]),
                task(
                    "b",
                    1,
                    vec![edge("a", Ternary::False), edge("a", Ternary::True)]
                ),
            ]
        )
        .is_err()
    );
}

#[test]
fn invalid_capacity_and_usage_fail_closed() {
    let tasks = vec![task("a", 0, vec![]), task("b", 0, vec![])];
    let mut state = snapshot(&tasks);
    let plan = Plan::new(7, tasks).unwrap();
    assert!(plan.select(&state, &limits(0)).is_err());
    state.observations.get_mut("a").unwrap().phase = Phase::InFlight;
    state.observations.get_mut("b").unwrap().phase = Phase::InFlight;
    assert!(plan.select(&state, &limits(1)).is_err());
    assert!(
        plan.select(
            &state,
            &Limits {
                capacity: Resources::new(1, 1),
                ..limits(2)
            }
        )
        .is_err()
    );
    state.observations.get_mut("a").unwrap().usage.output = 101;
    assert!(plan.select(&state, &limits(2)).is_err());
}

#[test]
fn snapshot_aggregate_overflow_is_not_wrapped_into_free_capacity() {
    let mut tasks = vec![task("a", 0, vec![]), task("b", 0, vec![])];
    tasks[0].max_tokens = u64::MAX;
    let mut state = snapshot(&tasks);
    for observation in state.observations.values_mut() {
        observation.phase = Phase::Settled;
    }
    state.observations.get_mut("a").unwrap().usage.cached = u64::MAX;
    state.observations.get_mut("b").unwrap().usage.fresh = 1;
    let plan = Plan::new(7, tasks).unwrap();
    assert!(
        plan.select(
            &state,
            &Limits {
                tokens: u64::MAX,
                ..limits(2)
            }
        )
        .is_err()
    );
}

#[test]
fn reverse_declared_chain_does_not_recurse_or_unlock_future_tasks() {
    let count = 1024;
    let tasks: Vec<_> = (0..count)
        .rev()
        .map(|i| {
            task(
                &format!("t{i:04}"),
                0,
                if i == 0 {
                    vec![]
                } else {
                    vec![edge(&format!("t{:04}", i - 1), Ternary::True)]
                },
            )
        })
        .collect();
    let state = snapshot(&tasks);
    let result = Plan::new(7, tasks)
        .unwrap()
        .select(&state, &limits(4))
        .unwrap();
    assert_eq!(result.selected, vec!["t0000"]);
    assert_eq!(result.decisions.len(), count);
}

#[test]
fn empty_plan_returns_empty_batch() {
    let plan = Plan::new(7, vec![]).unwrap();
    let state = Snapshot {
        revision: 7,
        observations: BTreeMap::new(),
    };
    let result = plan.select(&state, &limits(1)).unwrap();
    assert_eq!(result.selected.len(), 0);
    assert_eq!(result.decisions.len(), 0);
    assert_eq!(result.reserved_tokens, 0);
    assert_eq!(result.occupied_workers, 0);
}
