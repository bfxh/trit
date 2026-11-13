use trit_orch::{
    AdherenceViolation, Attempt, CheckReport, CheckResult, Effect, EffectSpec, EvidenceSpec,
    Ledger, Limits, ObligationKind, ObligationOutcome, Observation, Phase, Plan, Resources,
    Snapshot, Task, TaskBrief, TaskSpec, Ternary, TraceEvent, TraceEventKind, TraceLog,
    TracePayload, check_adherence, check_adherence_with_trace, task_brief,
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

fn brief_for(tasks: &[TaskSpec], ledger: &Ledger) -> TaskBrief {
    let plan = Plan::new(1, tasks.to_vec()).unwrap();
    let selection = plan
        .select_with_effects(&snapshot(tasks, 1), &limits(), ledger)
        .unwrap();
    task_brief(&plan, &selection, "login").unwrap()
}

fn settled(checks: u32, result: CheckResult) -> Observation {
    Observation {
        phase: Phase::Settled,
        evidence: Some(CheckReport {
            epoch: 0,
            checks,
            result,
        }),
        ..Observation::default()
    }
}

#[test]
fn compliant_brief_with_sufficient_evidence_passes() {
    let tasks = vec![task("login", vec![], 1)];
    let brief = brief_for(&tasks, &Ledger::new());

    let report = check_adherence(&brief, &settled(1, CheckResult::Passed));
    assert!(report.is_compliant(), "ORCH_ADHERENCE_OK");
    assert!(report.escalations.is_empty());
}

#[test]
fn settled_below_spec_escalates() {
    let tasks = vec![task("login", vec![], 2)];
    let brief = brief_for(&tasks, &Ledger::new());

    let report = check_adherence(&brief, &settled(1, CheckResult::Passed));
    assert_eq!(
        report.violations,
        [AdherenceViolation::EvidenceBelowSpec {
            required: 2,
            observed: 1
        }],
        "ORCH_ADHERENCE_EVIDENCE"
    );
    assert_eq!(report.escalations.len(), 1);
    assert_eq!(report.escalations[0].kind, ObligationKind::Escalate);
    assert_eq!(
        report.escalations[0].subject,
        "adherence:evidence_below_spec"
    );
}

#[test]
fn action_while_blocked_escalates() {
    let tasks = vec![task("login", vec![EffectSpec::new("login.write")], 1)];
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("login", 0, "login.write", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();
    let brief = brief_for(&tasks, &ledger);
    assert_eq!(brief.admission, Ternary::Unknown);
    assert_eq!(brief.obligations[0].kind, ObligationKind::Reconcile);

    let report = check_adherence(&brief, &settled(1, CheckResult::Passed));
    assert!(
        report.violations.iter().any(|violation| matches!(
            violation,
            AdherenceViolation::ActedWhileBlocked {
                admission: Ternary::Unknown
            }
        )),
        "ORCH_ADHERENCE_BLOCKED"
    );
    assert_eq!(
        report.escalations[0].subject,
        "adherence:acted_while_blocked"
    );
}

#[test]
fn trace_detects_actions_ignoring_open_obligations() {
    let tasks = vec![task("login", vec![EffectSpec::new("login.write")], 1)];
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("login", 0, "login.write", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();
    let brief = brief_for(&tasks, &ledger);
    let obligation = brief.obligations[0].clone();
    let obligation_id = obligation.id.clone();

    let mut log = TraceLog::new("t1");
    log.append(
        TraceEvent::new("t1", 1, TraceEventKind::EffectSent, "login", 100).with_payload(
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

    let report = check_adherence_with_trace(&brief, &Observation::default(), &log, 1);
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, AdherenceViolation::ActedWhileBlocked { .. })),
        "轨迹里的效果事件也算行动"
    );
    assert!(
        report.violations.iter().any(|violation| matches!(
            violation,
            AdherenceViolation::ObligationsIgnored { outstanding }
                if outstanding == std::slice::from_ref(&obligation_id)
        )),
        "ORCH_ADHERENCE_OBLIGATIONS"
    );

    log.append(
        TraceEvent::new("t1", 2, TraceEventKind::ObligationFulfilled, "login", 100).with_payload(
            TracePayload::Obligation {
                outcome: ObligationOutcome::Fulfilled,
                obligation,
            },
        ),
    )
    .unwrap();
    let report = check_adherence_with_trace(&brief, &Observation::default(), &log, 1);
    assert!(
        !report
            .violations
            .iter()
            .any(|violation| matches!(violation, AdherenceViolation::ObligationsIgnored { .. })),
        "义务已履行后不再报忽略"
    );
    assert!(
        report
            .violations
            .iter()
            .any(|violation| matches!(violation, AdherenceViolation::ActedWhileBlocked { .. }))
    );
}
