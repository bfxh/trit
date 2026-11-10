use std::collections::BTreeMap;

use trit_orch::{
    BlockReason, CheckReport, CheckResult, Dependency, Error, Limits, Observation, Phase, Plan,
    PlanPatch, Resources, Snapshot, Task, TaskSpec, Ternary,
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
    }
}

fn edge(id: &str) -> Dependency {
    Dependency {
        task: id.into(),
        required: Ternary::True,
    }
}

fn ready(tasks: &[TaskSpec]) -> Snapshot {
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
        capacity: Resources::new(4, 4),
        tokens: 10_000,
    }
}

fn patch(base: u64) -> PlanPatch {
    PlanPatch {
        base_revision: base,
        add: Vec::new(),
        remove: Vec::new(),
        replace: Vec::new(),
    }
}

fn settled(id: &str, snapshot: &mut Snapshot) {
    let observation = snapshot.observations.get_mut(id).unwrap();
    observation.phase = Phase::Settled;
    observation.evidence = Some(CheckReport {
        epoch: 0,
        checks: 1,
        result: CheckResult::Passed,
    });
}

#[test]
fn patch_requires_current_base_revision() {
    let tasks = vec![task("a", vec![])];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut stale = patch(6);
    stale.add.push(task("b", vec![]));
    assert_eq!(
        plan.patch(&stale, &ready(&tasks)),
        Err(Error::StalePatch {
            base: 6,
            current: 7
        }),
        "ORCH_PATCH_BASE_REVISION"
    );
}

#[test]
fn patch_adds_and_removes_tasks_and_bumps_revision() {
    let tasks = vec![task("a", vec![]), task("b", vec![])];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut change = patch(7);
    change.add.push(task("c", vec![edge("a")]));
    change.remove.push("b".into());
    let next = plan.patch(&change, &ready(&tasks)).unwrap();
    assert_eq!(next.revision(), 8);
    assert!(next.task("b").is_none());
    assert!(next.task("c").is_some());
    let mut state = ready(&tasks);
    state.revision = 8;
    state.observations.remove("b");
    state.observations.insert(
        "c".into(),
        Observation {
            authorization: Ternary::True,
            freshness: Ternary::True,
            ..Observation::default()
        },
    );
    assert_eq!(next.select(&state, &limits()).unwrap().selected, ["a"]);
}

#[test]
fn untouched_tasks_keep_evidence_across_a_patch() {
    let tasks = vec![task("a", vec![]), task("b", vec![edge("a")])];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut state = ready(&tasks);
    settled("a", &mut state);
    let mut change = patch(7);
    change.add.push(task("c", vec![]));
    let next = plan.patch(&change, &state).unwrap();
    let mut after = state.clone();
    after.revision = 8;
    after.observations.insert(
        "c".into(),
        Observation {
            authorization: Ternary::True,
            freshness: Ternary::True,
            ..Observation::default()
        },
    );
    assert_eq!(
        next.select(&after, &limits()).unwrap().selected,
        ["c", "b"],
        "ORCH_PATCH_PRESERVES_UNTOUCHED_EVIDENCE"
    );
}

#[test]
fn replaced_definition_bumps_epoch_and_runs_again() {
    let tasks = vec![task("a", vec![]), task("b", vec![edge("a")])];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut state = ready(&tasks);
    settled("a", &mut state);
    let mut change = patch(7);
    let mut revised = task("a", vec![]);
    revised.max_tokens = 50;
    change.replace.push(revised);
    let next = plan.patch(&change, &state).unwrap();
    assert_eq!(next.task("a").unwrap().epoch, 1, "ORCH_PATCH_EPOCH_BUMP");
    let mut after = state.clone();
    after.revision = 8;
    let selection = next.select(&after, &limits()).unwrap();
    assert_eq!(
        selection.selected,
        ["a"],
        "superseded task must be re-runnable"
    );
    let decision = selection.decisions.iter().find(|d| d.task == "a").unwrap();
    assert_eq!(
        decision.reasons,
        [BlockReason::Superseded {
            run_epoch: 0,
            task_epoch: 1
        }]
    );
    let downstream = selection.decisions.iter().find(|d| d.task == "b").unwrap();
    assert_eq!(downstream.admission, Ternary::Unknown);
}

#[test]
fn inflight_tasks_are_protected_from_removal_and_redefinition() {
    let tasks = vec![task("a", vec![]), task("b", vec![])];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut state = ready(&tasks);
    state.observations.get_mut("a").unwrap().phase = Phase::InFlight;

    let mut removal = patch(7);
    removal.remove.push("a".into());
    assert_eq!(
        plan.patch(&removal, &state),
        Err(Error::ProtectedInFlight { id: "a".into() }),
        "ORCH_PATCH_PROTECT_INFLIGHT"
    );

    let mut redefine = patch(7);
    let mut changed = task("a", vec![]);
    changed.max_tokens = 10;
    redefine.replace.push(changed);
    assert_eq!(
        plan.patch(&redefine, &state),
        Err(Error::ProtectedInFlight { id: "a".into() })
    );

    let mut same = patch(7);
    same.replace.push(task("a", vec![]));
    assert!(
        same.replace
            .iter()
            .all(|spec| spec.task.id == "a" && spec.max_tokens == 100)
    );
    assert!(
        plan.patch(&same, &state).is_ok(),
        "identical spec is allowed"
    );
    let mut allowed = patch(7);
    allowed.remove.push("b".into());
    assert!(
        plan.patch(&allowed, &state).is_ok(),
        "pending task may be removed"
    );
}

#[test]
fn patch_id_conflicts_and_missing_targets_are_rejected() {
    let tasks = vec![task("a", vec![])];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let state = ready(&tasks);

    let mut exists = patch(7);
    exists.add.push(task("a", vec![]));
    assert_eq!(
        plan.patch(&exists, &state),
        Err(Error::PatchAddExists { id: "a".into() })
    );

    let mut missing = patch(7);
    missing.remove.push("ghost".into());
    assert_eq!(
        plan.patch(&missing, &state),
        Err(Error::PatchTargetMissing { id: "ghost".into() })
    );

    let mut twice = patch(7);
    twice.remove.push("a".into());
    twice.add.push(task("a", vec![]));
    assert_eq!(
        plan.patch(&twice, &state),
        Err(Error::PatchDuplicate { id: "a".into() })
    );
}

#[test]
fn patch_cannot_leave_dangling_edges_or_cycles() {
    let tasks = vec![task("a", vec![]), task("b", vec![edge("a")])];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut removal = patch(7);
    removal.remove.push("a".into());
    assert!(matches!(
        plan.patch(&removal, &ready(&tasks)),
        Err(Error::UnknownDependency { .. })
    ));

    let mut cyclic = patch(7);
    cyclic.add.push(task("c", vec![edge("b")]));
    cyclic.replace.push(task("a", vec![edge("c")]));
    assert!(matches!(
        plan.patch(&cyclic, &ready(&tasks)),
        Err(Error::Cycle { .. })
    ));
}

#[test]
fn patch_requires_a_snapshot_of_the_current_plan() {
    let tasks = vec![task("a", vec![])];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut state = ready(&tasks);
    state.revision = 6;
    assert_eq!(
        plan.patch(&patch(7), &state),
        Err(Error::RevisionMismatch {
            expected: 7,
            actual: 6
        })
    );
    let mut missing = ready(&tasks);
    missing.observations.clear();
    assert!(matches!(
        plan.patch(&patch(7), &missing),
        Err(Error::ObservationMismatch { .. })
    ));
    assert_eq!(plan.revision(), 7, "failed patch must not mutate the plan");
}

#[test]
fn epoch_and_revision_overflow_fail_closed() {
    let mut maximum = task("a", vec![]);
    maximum.epoch = u64::MAX;
    let tasks = vec![maximum];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut change = patch(7);
    change.replace.push(task("a", vec![]));
    assert_eq!(
        plan.patch(&change, &ready(&tasks)),
        Err(Error::EpochOverflow { id: "a".into() }),
        "ORCH_PATCH_EPOCH_OVERFLOW"
    );

    let edge_plan = Plan::new(u64::MAX, vec![task("a", vec![])]).unwrap();
    let state = Snapshot {
        revision: u64::MAX,
        observations: BTreeMap::from([(
            "a".to_string(),
            Observation {
                authorization: Ternary::True,
                freshness: Ternary::True,
                ..Observation::default()
            },
        )]),
    };
    assert_eq!(
        edge_plan.patch(&patch(u64::MAX), &state),
        Err(Error::RevisionOverflow)
    );
}
