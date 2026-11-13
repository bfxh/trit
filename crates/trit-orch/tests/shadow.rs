use std::collections::BTreeMap;

use trit_orch::{
    Attempt, Baseline, Eager, Effect, Ledger, Limits, LongestFirst, Observation, Phase, Plan,
    Reconcile, Resources, ShadowPolicy, Snapshot, Task, TaskSpec, Ternary, TraceStep, replay,
};

fn task(id: &str, duration: u32) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration,
            deps: vec![],
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
        ..TaskSpec::default()
    }
}

fn step(ids: &[&str], in_doubt: &[&str]) -> TraceStep {
    let tasks: Vec<TaskSpec> = ids.iter().map(|id| task(id, 1)).collect();
    let mut observations = BTreeMap::new();
    for id in ids {
        let phase = if in_doubt.contains(id) {
            Phase::InFlight
        } else {
            Phase::Pending
        };
        observations.insert(
            (*id).to_string(),
            Observation {
                phase,
                authorization: Ternary::True,
                freshness: Ternary::True,
                ..Observation::default()
            },
        );
    }
    let mut ledger = Ledger::new();
    for id in in_doubt {
        let attempt: Attempt = ledger.prepare("t", 0, id, 1).unwrap();
        ledger.mark_sending(&attempt, 1).unwrap();
    }
    TraceStep {
        revision: 1,
        tasks,
        snapshot: Snapshot {
            revision: 1,
            observations,
        },
        limits: Limits {
            workers: 4,
            capacity: Resources::new(4, 4),
            tokens: 10_000,
        },
        ledger,
    }
}

#[test]
fn baseline_agrees_with_the_kernel_on_every_step() {
    let steps = vec![step(&["a", "b"], &[]), step(&["c"], &[])];
    let report = replay(&Baseline, &steps).unwrap();
    assert_eq!(report.steps, 2);
    assert_eq!(report.agreed_steps, 2, "ORCH_SHADOW_BASELINE");
    assert!(report.rejected.is_empty());
    assert!(report.is_safe());
}

#[test]
fn order_only_proposals_do_not_change_the_dispatched_set() {
    let steps = vec![step(&["a", "b", "c"], &[])];
    let report = replay(&LongestFirst, &steps).unwrap();
    assert_eq!(report.rejected, Vec::<String>::new());
    assert_eq!(report.skipped, Vec::<String>::new());
    assert_eq!(report.order_mismatches, 1, "ORCH_SHADOW_ORDER");
    assert!(report.is_safe(), "顺序建议只应记差异，不算危险");
    assert_eq!(report.agreed_steps, 0);
}

#[test]
fn eager_with_nothing_extra_stays_safe() {
    let steps = vec![step(&["a"], &[])];
    let report = replay(&Eager, &steps).unwrap();
    assert!(report.rejected.is_empty());
    assert!(report.is_safe());
    assert_eq!(report.agreed_steps, 1);
}

#[test]
fn proposing_to_resend_an_in_doubt_effect_is_flagged_unsafe() {
    // a 在途：内核不会选它；eager 把它当“额外派发”提出来 → 必须判危险。
    let report = replay(&Eager, &[step(&["a"], &["a"])]).unwrap();
    assert_eq!(report.rejected, vec!["0:a"]);
    assert_eq!(report.unsafe_extra, vec!["0:a"], "ORCH_SHADOW_UNSAFE");
    assert!(!report.is_safe());
}

#[test]
fn reconciled_effect_is_still_unsafe_to_resend() {
    let mut trace = step(&["a"], &[]);
    let attempt = trace.ledger.prepare("t", 0, "a", 1).unwrap();
    trace.ledger.mark_sending(&attempt, 1).unwrap();
    trace.ledger.mark_unknown(&attempt).unwrap();
    trace
        .ledger
        .reconcile(&attempt, Reconcile::Present)
        .unwrap();
    assert_eq!(trace.ledger.state("a"), Some(Effect::Confirmed));
    trace.snapshot.observations.get_mut("a").unwrap().phase = Phase::Settled;
    let report = replay(&Eager, &[trace]).unwrap();
    assert_eq!(report.unsafe_extra, vec!["0:a"], "ORCH_SHADOW_UNSAFE");
    assert!(!report.is_safe());
}

#[test]
fn replay_is_deterministic_and_never_mutates_inputs() {
    let steps = vec![step(&["a", "b"], &[])];
    let before = steps.clone();
    let first = replay(&LongestFirst, &steps).unwrap();
    let second = replay(&LongestFirst, &steps).unwrap();
    assert_eq!(first, second, "ORCH_SHADOW_DETERMINISTIC");
    assert_eq!(steps, before, "回放不得改输入");
}

#[test]
fn policy_names_are_stable() {
    assert_eq!(Baseline.name(), "baseline");
    assert_eq!(LongestFirst.name(), "longest_first");
    assert_eq!(Eager.name(), "eager");
}

fn task_with(id: &str, duration: u32, deps: Vec<(&str, Ternary)>) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration,
            deps: deps
                .into_iter()
                .map(|(task, required)| trit_orch::Dependency {
                    task: task.into(),
                    required,
                })
                .collect(),
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
        ..TaskSpec::default()
    }
}

fn ready_step(tasks: Vec<TaskSpec>) -> TraceStep {
    let ids: Vec<String> = tasks.iter().map(|spec| spec.task.id.clone()).collect();
    let observations = ids
        .into_iter()
        .map(|id| {
            (
                id,
                Observation {
                    authorization: Ternary::True,
                    freshness: Ternary::True,
                    ..Observation::default()
                },
            )
        })
        .collect();
    TraceStep {
        revision: 1,
        tasks,
        snapshot: Snapshot {
            revision: 1,
            observations,
        },
        limits: Limits {
            workers: 8,
            capacity: Resources::new(8, 8),
            tokens: 10_000,
        },
        ledger: Ledger::new(),
    }
}

#[test]
fn critical_path_prioritises_the_deeper_chain() {
    // m 解锁 3 步链；a 与 z 是叶子（z 更长，内核按 duration 先选 z）。
    let build = || {
        vec![
            task_with("m", 1, vec![]),
            task_with("a", 1, vec![]),
            task_with("z", 1, vec![]),
            task_with("m2", 1, vec![("m", Ternary::True)]),
            task_with("m3", 1, vec![("m2", Ternary::True)]),
        ]
    };
    let levels = trit_orch::bottom_levels(&build());
    assert_eq!(levels["m"], 3, "ORCH_SHADOW_CP_LEVELS");
    assert_eq!(levels["m2"], 2);
    assert_eq!(levels["z"], 1);

    // 直接检查建议本身：顺序只能是 m 先，且不增删任务。
    let trace = ready_step(build());
    let plan = Plan::new(trace.revision, trace.tasks.clone()).unwrap();
    let selection = plan.select(&trace.snapshot, &trace.limits).unwrap();
    assert_eq!(
        selection.selected,
        vec!["z", "m", "a"],
        "内核按 duration/id 排"
    );
    let context = trit_orch::ShadowContext {
        selection: &selection,
        tasks: &trace.tasks,
        ledger: &trace.ledger,
        limits: &trace.limits,
    };
    let proposal = trit_orch::CriticalPath.propose(&context);
    assert_eq!(
        proposal.order.as_deref(),
        Some(&["m".to_string(), "a".to_string(), "z".to_string()][..]),
        "ORCH_SHADOW_CP_ORDER"
    );

    let report = replay(&trit_orch::CriticalPath, &[trace]).unwrap();
    assert_eq!(report.rejected, Vec::<String>::new());
    assert_eq!(report.order_mismatches, 1);
    assert!(report.is_safe());
}

#[test]
fn critical_path_counts_optional_edges_but_not_unknown_ones() {
    let optional = vec![
        task_with("a", 1, vec![]),
        task_with("tail", 5, vec![("a", Ternary::False)]),
    ];
    let levels = trit_orch::bottom_levels(&optional);
    assert_eq!(
        levels["a"], 6,
        "ORCH_SHADOW_CP_EDGES: 可选边预计会跑，要计入"
    );

    let unknown = vec![
        task_with("a", 1, vec![]),
        task_with("tail", 5, vec![("a", Ternary::Unknown)]),
    ];
    let levels = trit_orch::bottom_levels(&unknown);
    assert_eq!(levels["a"], 1, "ORCH_SHADOW_CP_EDGES");
}

#[test]
fn critical_path_order_is_deterministic_and_keeps_the_set() {
    let build = || {
        vec![
            task_with("a", 1, vec![]),
            task_with("b", 1, vec![]),
            task_with("c", 1, vec![]),
            task_with("a2", 1, vec![("a", Ternary::True)]),
            task_with("a3", 1, vec![("a2", Ternary::True)]),
        ]
    };
    let first = replay(&trit_orch::CriticalPath, &[ready_step(build())]).unwrap();
    let second = replay(&trit_orch::CriticalPath, &[ready_step(build())]).unwrap();
    assert_eq!(first, second, "ORCH_SHADOW_CP_DETERMINISTIC");
    assert_eq!(first.rejected, Vec::<String>::new());
    assert_eq!(first.skipped, Vec::<String>::new());
}

#[test]
fn critical_path_handles_a_long_chain_without_recursion() {
    let mut tasks = vec![task_with("t0000", 1, vec![])];
    for index in 1..5_000u32 {
        let previous = format!("t{:04}", index - 1);
        tasks.push(task_with(
            &format!("t{index:04}"),
            1,
            vec![(previous.as_str(), Ternary::True)],
        ));
    }
    let levels = trit_orch::bottom_levels(&tasks);
    assert_eq!(levels["t0000"], 5_000, "ORCH_SHADOW_CP_LONG_CHAIN");
}
