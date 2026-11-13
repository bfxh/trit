use trit_orch::{
    BlockReason, Effect, EffectSpec, Error, Ledger, Limits, ObligationKind, Observation, Plan,
    Reconcile, Resources, Snapshot, Task, TaskSpec, Ternary, dispatch_ready, effect_operations,
    prepare_effects,
};

fn task(id: &str, effects: Vec<EffectSpec>) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        effects,
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
        ..TaskSpec::default()
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

fn limits() -> Limits {
    Limits {
        workers: 2,
        capacity: Resources::new(2, 2),
        tokens: 10_000,
    }
}

#[test]
fn declared_effects_gate_every_operation() {
    let tasks = vec![task(
        "deploy",
        vec![
            EffectSpec::new("deploy.api"),
            EffectSpec::new("deploy.docs"),
        ],
    )];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let snap = snapshot(&tasks);
    let mut ledger = Ledger::new();
    let docs = ledger.prepare("deploy", 0, "deploy.docs", 1).unwrap();
    ledger.mark_sending(&docs, 1).unwrap();
    let api = ledger.prepare("deploy", 0, "deploy.api", 2).unwrap();
    ledger.mark_sending(&api, 2).unwrap();

    let selection = plan.select_with_effects(&snap, &limits(), &ledger).unwrap();
    assert!(selection.selected.is_empty(), "ORCH_EFFECT_SPEC_GATE");
    let decision = &selection.decisions[0];
    assert_eq!(decision.admission, Ternary::Unknown);
    assert!(
        decision.reasons.iter().any(|reason| matches!(
            reason,
            BlockReason::Effect {
                state: Effect::Sending
            }
        )),
        "ORCH_EFFECT_SPEC_GATE: 任一操作在途都要挡下整任务"
    );
    assert_eq!(
        decision.obligations.len(),
        1,
        "多个在途操作只生成一条对账义务"
    );
    assert_eq!(decision.obligations[0].kind, ObligationKind::Reconcile);

    ledger.reconcile(&docs, Reconcile::Absent).unwrap();
    ledger.reconcile(&api, Reconcile::Absent).unwrap();
    let selection = plan.select_with_effects(&snap, &limits(), &ledger).unwrap();
    assert_eq!(selection.selected, ["deploy"]);
}

#[test]
fn empty_effect_list_falls_back_to_task_id() {
    let tasks = vec![task("deploy", Vec::new())];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("deploy", 0, "deploy", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();

    let selection = plan
        .select_with_effects(&snapshot(&tasks), &limits(), &ledger)
        .unwrap();
    assert!(selection.selected.is_empty(), "ORCH_EFFECT_SPEC_FALLBACK");
    assert_eq!(effect_operations(&tasks[0]), ["deploy"]);
}

#[test]
fn dispatch_requires_every_declared_operation() {
    let tasks = vec![task(
        "deploy",
        vec![
            EffectSpec::new("deploy.api"),
            EffectSpec::new("deploy.docs"),
        ],
    )];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let selection = plan
        .select_with_effects(&snapshot(&tasks), &limits(), &Ledger::new())
        .unwrap();
    assert_eq!(selection.selected, ["deploy"]);

    let mut ledger = Ledger::new();
    ledger.prepare("deploy", 0, "deploy.api", 1).unwrap();
    assert!(
        matches!(
            dispatch_ready(&selection, &tasks, &ledger),
            Err(Error::NotPrepared { operation, .. }) if operation == "deploy.docs"
        ),
        "ORCH_EFFECT_SPEC_DISPATCH: 少一个操作的意图就不能派发"
    );

    ledger.prepare("deploy", 0, "deploy.docs", 2).unwrap();
    let ready = dispatch_ready(&selection, &tasks, &ledger).unwrap();
    assert_eq!(
        ready
            .iter()
            .map(|attempt| attempt.operation.as_str())
            .collect::<Vec<_>>(),
        ["deploy.api", "deploy.docs"]
    );
}

#[test]
fn duplicate_operations_fail_at_plan_construction() {
    let duplicate = vec![task(
        "deploy",
        vec![EffectSpec::new("pay"), EffectSpec::new("pay")],
    )];
    assert_eq!(
        Plan::new(1, duplicate),
        Err(Error::DuplicateEffectOperation {
            task: "deploy".into(),
            operation: "pay".into()
        }),
        "ORCH_EFFECT_SPEC_DUPLICATE"
    );

    let normalized = vec![task(
        "deploy",
        vec![EffectSpec::default(), EffectSpec::new("deploy")],
    )];
    assert_eq!(
        Plan::new(1, normalized),
        Err(Error::DuplicateEffectOperation {
            task: "deploy".into(),
            operation: "deploy".into()
        })
    );
}

#[test]
fn confirmed_declared_operation_blocks_without_obligation() {
    let tasks = vec![task("deploy", vec![EffectSpec::new("deploy.api")])];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("deploy", 0, "deploy.api", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();
    ledger.reconcile(&attempt, Reconcile::Present).unwrap();

    let selection = plan
        .select_with_effects(&snapshot(&tasks), &limits(), &ledger)
        .unwrap();
    let decision = &selection.decisions[0];
    assert_eq!(
        decision.admission,
        Ternary::False,
        "ORCH_EFFECT_SPEC_CONFIRMED"
    );
    assert!(decision.obligations.is_empty());
}

#[test]
fn prepare_effects_records_every_operation() {
    let tasks = [task(
        "deploy",
        vec![
            EffectSpec::new("deploy.api"),
            EffectSpec::new("deploy.docs"),
        ],
    )];
    let mut ledger = Ledger::new();
    let attempts = prepare_effects(&mut ledger, &tasks[0], 7).unwrap();
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| attempt.operation.as_str())
            .collect::<Vec<_>>(),
        ["deploy.api", "deploy.docs"]
    );
    assert!(attempts.iter().all(|attempt| attempt.task == "deploy"
        && attempt.epoch == 0
        && attempt.fence == 7
        && attempt.attempt == 1));
    assert_eq!(ledger.state("deploy.api"), Some(Effect::Prepared));
    assert_eq!(ledger.state("deploy.docs"), Some(Effect::Prepared));
}

#[test]
fn prepare_effects_is_all_or_nothing() {
    let tasks = [task(
        "deploy",
        vec![EffectSpec::new("pay"), EffectSpec::new("ship")],
    )];

    let mut in_doubt = Ledger::new();
    let ship = in_doubt.prepare("deploy", 0, "ship", 1).unwrap();
    in_doubt.mark_sending(&ship, 1).unwrap();
    assert_eq!(
        prepare_effects(&mut in_doubt, &tasks[0], 2),
        Err(Error::EffectInDoubt {
            operation: "ship".into()
        }),
        "ORCH_PREPARE_EFFECTS_ATOMIC"
    );
    assert_eq!(in_doubt.state("pay"), None);
    assert_eq!(in_doubt.state("ship"), Some(Effect::Sending));

    let mut stale = Ledger::new();
    let ship = stale.prepare("deploy", 0, "ship", 5).unwrap();
    stale.mark_sending(&ship, 5).unwrap();
    stale.record_result(&ship, false).unwrap();
    assert_eq!(
        stale.state("ship"),
        Some(Effect::Refuted),
        "Refuted 可重试，才能走到 fence 校验"
    );
    assert_eq!(
        prepare_effects(&mut stale, &tasks[0], 3),
        Err(Error::StaleFence {
            operation: "ship".into(),
            given: 3,
            current: 5
        })
    );
    assert_eq!(stale.state("pay"), None);

    let mut done = Ledger::new();
    let ship = done.prepare("deploy", 0, "ship", 1).unwrap();
    done.mark_sending(&ship, 1).unwrap();
    done.reconcile(&ship, Reconcile::Present).unwrap();
    assert_eq!(
        prepare_effects(&mut done, &tasks[0], 2),
        Err(Error::EffectAlreadyApplied {
            operation: "ship".into()
        })
    );
    assert_eq!(done.state("pay"), None);
}

#[test]
fn prepare_effects_then_dispatch_matches() {
    let tasks = vec![task(
        "deploy",
        vec![
            EffectSpec::new("deploy.api"),
            EffectSpec::new("deploy.docs"),
        ],
    )];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let selection = plan
        .select_with_effects(&snapshot(&tasks), &limits(), &Ledger::new())
        .unwrap();
    let mut ledger = Ledger::new();
    let prepared = prepare_effects(&mut ledger, &tasks[0], 7).unwrap();
    let ready = dispatch_ready(&selection, &tasks, &ledger).unwrap();
    assert_eq!(ready, prepared, "ORCH_PREPARE_EFFECTS_DISPATCH");
}

#[test]
fn prepare_effects_falls_back_to_task_id() {
    let tasks = [task("deploy", Vec::new())];
    let mut ledger = Ledger::new();
    let attempts = prepare_effects(&mut ledger, &tasks[0], 1).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].operation, "deploy");
    assert_eq!(ledger.state("deploy"), Some(Effect::Prepared));
}
