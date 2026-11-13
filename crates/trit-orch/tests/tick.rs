use trit_orch::{
    AdherenceViolation, Attempt, CheckReport, CheckResult, Effect, EffectSpec, EvidenceSpec,
    InterventionPolicy, Ledger, Limits, ObligationKind, Observation, Phase, Plan, PlanPatch,
    Resources, Snapshot, Task, TaskSpec, Ternary, TickInput, TraceEvent, TraceEventKind, TraceLog,
    TracePayload, tick,
};

static POLICY: InterventionPolicy = InterventionPolicy {
    quarantine_unsafe: true,
};

fn task(id: &str, effects: Vec<EffectSpec>, min_assertions: u32) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        produces: EvidenceSpec { min_assertions },
        effects,
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
fn tick_briefs_selected_tasks_and_collects_obligations() {
    let tasks = vec![
        task("a", vec![], 0),
        task("b", vec![EffectSpec::new("b.write")], 0),
    ];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let snapshot = snapshot(&tasks, 1);
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("b", 0, "b.write", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();

    let result = tick(TickInput {
        plan: &plan,
        snapshot: &snapshot,
        limits: &limits(),
        ledger: &ledger,
        claims: None,
        briefed: &[],
        trace: None,
        since_seq: 0,
        intervention: &POLICY,
    })
    .unwrap();

    assert_eq!(result.revision, 1);
    assert_eq!(result.selection.selected, ["a"]);
    assert_eq!(result.briefs.len(), 1);
    assert_eq!(result.briefs[0].task, "a");
    assert!(result.adherence.is_empty());
    assert_eq!(result.guidance_metrics.briefs, 0);
    assert_eq!(result.obligations.len(), 1);
    assert_eq!(result.obligations[0].kind, ObligationKind::Reconcile);
    assert_eq!(result.obligations[0].task, "b");
}

#[test]
fn tick_reports_adherence_against_previous_briefs() {
    let tasks = vec![task("login", vec![], 2)];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let pending = snapshot(&tasks, 1);
    let first = tick(TickInput {
        plan: &plan,
        snapshot: &pending,
        limits: &limits(),
        ledger: &Ledger::new(),
        claims: None,
        briefed: &[],
        trace: None,
        since_seq: 0,
        intervention: &POLICY,
    })
    .unwrap();
    assert_eq!(first.briefs.len(), 1);

    let mut settled = pending.clone();
    let observation = settled.observations.get_mut("login").unwrap();
    observation.phase = Phase::Settled;
    observation.evidence = Some(CheckReport {
        epoch: 0,
        checks: 1,
        result: CheckResult::Passed,
    });

    let second = tick(TickInput {
        plan: &plan,
        snapshot: &settled,
        limits: &limits(),
        ledger: &Ledger::new(),
        claims: None,
        briefed: &first.briefs,
        trace: None,
        since_seq: 0,
        intervention: &POLICY,
    })
    .unwrap();

    assert!(second.selection.selected.is_empty());
    assert!(second.briefs.is_empty());
    assert_eq!(second.adherence.len(), 1);
    assert_eq!(
        second.adherence[0].violations,
        [AdherenceViolation::EvidenceBelowSpec {
            required: 2,
            observed: 1
        }],
        "ORCH_TICK_ADHERENCE"
    );
    assert_eq!(second.guidance_metrics.briefs, 1);
    assert_eq!(second.guidance_metrics.tasks_with_violations, 1);
    assert_eq!(
        second.guidance_metrics.violation_rate_bp(),
        10_000,
        "ORCH_TICK_GUIDANCE_RATE"
    );
    assert!(second.obligations.iter().any(|obligation| {
        obligation.kind == ObligationKind::Measure
            && obligation.subject == "intervention:evidence_gap"
    }));
}

#[test]
fn tick_skips_briefs_from_superseded_epochs() {
    let tasks = vec![task("login", vec![], 1)];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let pending = snapshot(&tasks, 1);
    let first = tick(TickInput {
        plan: &plan,
        snapshot: &pending,
        limits: &limits(),
        ledger: &Ledger::new(),
        claims: None,
        briefed: &[],
        trace: None,
        since_seq: 0,
        intervention: &POLICY,
    })
    .unwrap();

    let mut revised = task("login", vec![], 1);
    revised.max_tokens = 50;
    let patch = PlanPatch {
        base_revision: 1,
        add: vec![],
        remove: vec![],
        replace: vec![revised],
    };
    let next = plan.patch(&patch, &pending).unwrap();
    assert_eq!(next.task("login").unwrap().epoch, 1);

    let after = snapshot(next.tasks(), 2);
    let result = tick(TickInput {
        plan: &next,
        snapshot: &after,
        limits: &limits(),
        ledger: &Ledger::new(),
        claims: None,
        briefed: &first.briefs,
        trace: None,
        since_seq: 0,
        intervention: &POLICY,
    })
    .unwrap();

    assert!(result.adherence.is_empty(), "旧 epoch 的简报不作遵循度判定");
    assert_eq!(result.briefs.len(), 1);
    assert_eq!(result.briefs[0].epoch, 1);
}

#[test]
fn tick_uses_trace_to_catch_post_brief_actions() {
    let tasks = vec![task("login", vec![EffectSpec::new("login.write")], 1)];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let pending = snapshot(&tasks, 1);
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("login", 0, "login.write", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();

    let selection = plan
        .select_with_effects(&pending, &limits(), &ledger)
        .unwrap();
    let brief = trit_orch::task_brief(&plan, &selection, "login").unwrap();

    let mut log = TraceLog::new("t1");
    log.append(
        TraceEvent::new("t1", 5, TraceEventKind::EffectSent, "login", 100).with_payload(
            TracePayload::Effect {
                attempt: Attempt {
                    operation: "login.write".into(),
                    task: "login".into(),
                    epoch: 0,
                    attempt: 1,
                    fence: 1,
                },
                state: Effect::Sent,
            },
        ),
    )
    .unwrap();

    let result = tick(TickInput {
        plan: &plan,
        snapshot: &pending,
        limits: &limits(),
        ledger: &ledger,
        claims: None,
        briefed: std::slice::from_ref(&brief),
        trace: Some(&log),
        since_seq: 5,
        intervention: &POLICY,
    })
    .unwrap();

    assert!(
        result.adherence.iter().any(|report| report
            .violations
            .iter()
            .any(|violation| matches!(violation, AdherenceViolation::ActedWhileBlocked { .. }))),
        "ORCH_TICK_TRACE"
    );
    assert!(result.obligations.iter().any(|obligation| {
        obligation.kind == ObligationKind::Escalate
            && obligation.subject == "intervention:quarantine"
    }));
    assert!(
        result
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Reconcile)
    );
}
