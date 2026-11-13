use trit_orch::{
    AdmissionObligation, BlockReason, Effect, EffectSpec, Error, EvidenceSpec, Ledger, Limits,
    ObligationKind, ObligationTaskPolicy, Observation, Plan, Resources, Snapshot, Task,
    TaskGuidance, TaskSpec, Ternary, plan_patch_for_obligations, task_brief,
};

fn task(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        ..TaskSpec::default()
    }
}

fn snapshot(tasks: &[TaskSpec], revision: u64) -> Snapshot {
    Snapshot {
        revision,
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
fn brief_carries_guidance_and_evidence_contract() {
    let tasks = vec![TaskSpec {
        task: Task {
            id: "login".into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        guidance: TaskGuidance {
            objective: "实现登录".into(),
            constraints: vec!["不得改 schema".into()],
            stop_when: vec!["权限不明就停".into()],
        },
        resolves: vec!["auth".into()],
        produces: EvidenceSpec { min_assertions: 2 },
        effects: vec![EffectSpec::new("login.write")],
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
    }];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let selection = plan
        .select_with_effects(&snapshot(&tasks, 1), &limits(), &Ledger::new())
        .unwrap();

    let brief = task_brief(&plan, &selection, "login").unwrap();
    assert_eq!(brief.guidance.objective, "实现登录");
    assert_eq!(brief.guidance.constraints, ["不得改 schema"]);
    assert_eq!(brief.guidance.stop_when, ["权限不明就停"]);
    assert_eq!(brief.resolves, ["auth"]);
    assert_eq!(brief.produces.min_assertions, 2);
    assert_eq!(brief.effects, [EffectSpec::new("login.write")]);
    assert_eq!(brief.admission, Ternary::True);
    assert_eq!(brief.granted_tokens, selection.decisions[0].granted_tokens);
    assert!(brief.blockers.is_empty());
    assert!(brief.obligations.is_empty());
}

#[test]
fn blocked_brief_carries_blockers_and_obligations() {
    let tasks = vec![task("login")];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("login", 0, "login", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();
    let selection = plan
        .select_with_effects(&snapshot(&tasks, 1), &limits(), &ledger)
        .unwrap();

    let brief = task_brief(&plan, &selection, "login").unwrap();
    assert_eq!(brief.admission, Ternary::Unknown);
    assert!(
        brief.guidance.objective.is_empty(),
        "未声明的引导由宿主解释"
    );
    assert!(brief.blockers.iter().any(|reason| matches!(
        reason,
        BlockReason::Effect {
            state: Effect::Sending
        }
    )));
    assert_eq!(brief.obligations[0].kind, ObligationKind::Reconcile);
    assert_eq!(brief.granted_tokens, 0);
}

#[test]
fn generated_obligation_task_brief_explains_the_work() {
    let tasks = vec![task("login")];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let before = snapshot(&tasks, 7);
    let obligation =
        AdmissionObligation::new(ObligationKind::Measure, "login", "auth", "验证登录断言");
    let result = plan_patch_for_obligations(
        &plan,
        &before,
        [obligation],
        &ObligationTaskPolicy::default(),
    );
    let generated = result.patch.add[0].task.id.clone();
    let next = plan.patch(&result.patch, &before).unwrap();
    let after_tasks = next.tasks().to_vec();
    let selection = next.select(&snapshot(&after_tasks, 8), &limits()).unwrap();

    let brief = task_brief(&next, &selection, &generated).unwrap();
    assert_eq!(brief.guidance.objective, "验证登录断言");
    assert_eq!(brief.resolves, ["login"]);
    assert_eq!(brief.produces.min_assertions, 1);
    assert_eq!(brief.admission, Ternary::True);
}

#[test]
fn brief_fails_closed_on_missing_task_or_stale_selection() {
    let tasks = vec![task("login")];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let selection = plan.select(&snapshot(&tasks, 1), &limits()).unwrap();
    assert!(matches!(
        task_brief(&plan, &selection, "ghost"),
        Err(Error::BriefIncomplete { task, .. }) if task == "ghost"
    ));

    let stale_plan = Plan::new(2, tasks.clone()).unwrap();
    assert_eq!(
        task_brief(&stale_plan, &selection, "login"),
        Err(Error::RevisionMismatch {
            expected: 2,
            actual: 1
        }),
        "ORCH_BRIEF_REVISION"
    );
}
