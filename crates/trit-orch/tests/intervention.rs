use trit_orch::{
    AdherenceReport, AdherenceViolation, InterventionAction, InterventionPolicy, ObligationKind,
    Ternary, plan_interventions,
};

fn report(task: &str, violations: Vec<AdherenceViolation>) -> AdherenceReport {
    AdherenceReport {
        task: task.into(),
        violations,
        escalations: Vec::new(),
    }
}

#[test]
fn groups_safety_violations_and_holds_tasks() {
    let reports = vec![
        report(
            "b",
            vec![AdherenceViolation::ActedWhileBlocked {
                admission: Ternary::Unknown,
            }],
        ),
        report(
            "a",
            vec![AdherenceViolation::ActedWhileBlocked {
                admission: Ternary::False,
            }],
        ),
    ];

    let plan = plan_interventions(&reports, &InterventionPolicy::default());
    assert_eq!(plan.held, ["a", "b"], "ORCH_INTERVENTION_HOLD");
    assert_eq!(plan.interventions.len(), 1);
    assert_eq!(plan.interventions[0].action, InterventionAction::Quarantine);
    assert_eq!(plan.interventions[0].tasks, ["a", "b"]);
    assert_eq!(plan.obligations.len(), 1);
    assert_eq!(plan.obligations[0].kind, ObligationKind::Escalate);
    assert_eq!(plan.obligations[0].task, "a");
    assert_eq!(plan.obligations[0].subject, "intervention:quarantine");
}

#[test]
fn held_tasks_suppress_lower_priority_interventions() {
    let reports = vec![
        report(
            "a",
            vec![
                AdherenceViolation::ActedWhileBlocked {
                    admission: Ternary::Unknown,
                },
                AdherenceViolation::EvidenceBelowSpec {
                    required: 2,
                    observed: 1,
                },
            ],
        ),
        report(
            "b",
            vec![AdherenceViolation::EvidenceBelowSpec {
                required: 3,
                observed: 1,
            }],
        ),
    ];

    let plan = plan_interventions(&reports, &InterventionPolicy::default());
    assert_eq!(plan.held, ["a"]);
    assert_eq!(plan.interventions.len(), 2, "安全组 + 未隔离任务的补证");
    assert_eq!(
        plan.interventions[1].action,
        InterventionAction::RequireEvidence
    );
    assert_eq!(plan.interventions[1].tasks, ["b"]);
    assert_eq!(plan.obligations[1].kind, ObligationKind::Measure);
    assert_eq!(plan.obligations[1].subject, "intervention:evidence_gap");
    assert_eq!(
        plan.obligations.iter().filter(|o| o.task == "a").count(),
        1,
        "被隔离的任务不再叠加补证义务"
    );
}

#[test]
fn ignored_obligations_group_into_one_escalation() {
    let reports = vec![
        report(
            "a",
            vec![AdherenceViolation::ObligationsIgnored {
                outstanding: vec!["reconcile:a:pay".into()],
            }],
        ),
        report(
            "b",
            vec![AdherenceViolation::ObligationsIgnored {
                outstanding: vec!["claim:b:claim".into()],
            }],
        ),
    ];

    let plan = plan_interventions(&reports, &InterventionPolicy::default());
    assert_eq!(plan.interventions.len(), 1);
    assert_eq!(plan.interventions[0].action, InterventionAction::Escalate);
    assert_eq!(plan.interventions[0].tasks, ["a", "b"]);
    assert_eq!(
        plan.obligations[0].subject,
        "intervention:obligations_ignored"
    );
}

#[test]
fn quarantine_can_be_disabled() {
    let reports = vec![report(
        "a",
        vec![AdherenceViolation::ActedWhileBlocked {
            admission: Ternary::Unknown,
        }],
    )];
    let policy = InterventionPolicy {
        quarantine_unsafe: false,
    };
    let plan = plan_interventions(&reports, &policy);
    assert!(plan.held.is_empty(), "ORCH_INTERVENTION_NO_HOLD");
    assert_eq!(plan.interventions[0].action, InterventionAction::Escalate);
    assert_eq!(
        plan.obligations[0].subject,
        "intervention:acted_while_blocked"
    );
}
