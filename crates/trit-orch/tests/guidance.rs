use trit_orch::{
    AdherenceReport, AdherenceViolation, EffectSpec, EvidenceSpec, GuidanceMetrics, Ledger, Limits,
    Observation, Plan, Resources, Snapshot, Task, TaskBrief, TaskGuidance, TaskSpec, Ternary,
    guidance_from_text, render_brief, task_brief,
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
        workers: 4,
        capacity: Resources::new(100, 100),
        tokens: 10_000,
    }
}

fn brief_for(tasks: &[TaskSpec], ledger: &Ledger) -> TaskBrief {
    let plan = Plan::new(1, tasks.to_vec()).unwrap();
    let selection = plan
        .select_with_effects(&snapshot(tasks), &limits(), ledger)
        .unwrap();
    task_brief(&plan, &selection, &tasks[0].task.id).unwrap()
}

#[test]
fn extracts_guidance_from_section_headers() {
    let guidance = guidance_from_text(
        "目标：实现登录\n约束：\n- 不得改 schema\n- 必须跑测试\n停止：权限不明就停",
    );
    assert_eq!(guidance.objective, "实现登录");
    assert_eq!(guidance.constraints, ["不得改 schema", "必须跑测试"]);
    assert_eq!(guidance.stop_when, ["权限不明就停"]);
}

#[test]
fn extracts_guidance_from_plain_sentences() {
    let guidance = guidance_from_text("实现登录。必须跑测试，禁止改 schema。一旦权限不明就停。");
    assert_eq!(guidance.objective, "实现登录");
    assert_eq!(guidance.constraints, ["必须跑测试，禁止改 schema"]);
    assert_eq!(
        guidance.stop_when,
        ["一旦权限不明就停"],
        "ORCH_GUIDANCE_TEXT"
    );
}

#[test]
fn english_headers_are_case_insensitive() {
    let guidance = guidance_from_text(
        "Objective: ship login\nConstraints: must run tests\nStop when: permission unknown",
    );
    assert_eq!(guidance.objective, "ship login");
    assert_eq!(guidance.constraints, ["must run tests"]);
    assert_eq!(guidance.stop_when, ["permission unknown"]);
}

#[test]
fn empty_text_yields_empty_guidance() {
    assert_eq!(guidance_from_text("  \n"), TaskGuidance::default());
}

#[test]
fn renders_brief_with_guidance_and_obligations() {
    let tasks = vec![TaskSpec {
        guidance: TaskGuidance {
            objective: "实现登录".into(),
            constraints: vec!["不得改 schema".into()],
            stop_when: vec!["权限不明就停".into()],
        },
        ..task("login", vec![EffectSpec::new("login.write")], 2)
    }];
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("login", 0, "login.write", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();

    let brief = brief_for(&tasks, &Ledger::new());
    let text = render_brief(&brief);
    assert!(text.contains("# Task login (epoch 0)"));
    assert!(text.contains("## Objective\n实现登录"));
    assert!(text.contains("- 不得改 schema"));
    assert!(text.contains("min_assertions: 2"));
    assert!(text.contains("- login.write"));
    assert!(text.contains("state: True"));

    let blocked = brief_for(&tasks, &ledger);
    let text = render_brief(&blocked);
    assert!(text.contains("state: Unknown"), "ORCH_GUIDANCE_RENDER");
    assert!(text.contains("blocker: effect: Sending"));
    assert!(text.contains("[reconcile]"));
}

#[test]
fn renders_task_id_fallback_when_effects_are_undeclared() {
    let tasks = vec![task("login", vec![], 1)];
    let brief = brief_for(&tasks, &Ledger::new());
    let text = render_brief(&brief);
    assert!(
        text.contains("- login (task id fallback)"),
        "ORCH_GUIDANCE_EFFECT_FALLBACK"
    );
}

#[test]
fn metrics_aggregate_violation_rates() {
    let mut metrics = GuidanceMetrics::default();
    assert_eq!(metrics.violation_rate_bp(), 0);

    let reports = [
        AdherenceReport {
            task: "a".into(),
            violations: Vec::new(),
            escalations: Vec::new(),
        },
        AdherenceReport {
            task: "b".into(),
            violations: vec![
                AdherenceViolation::ActedWhileBlocked {
                    admission: Ternary::Unknown,
                },
                AdherenceViolation::EvidenceBelowSpec {
                    required: 2,
                    observed: 1,
                },
            ],
            escalations: Vec::new(),
        },
    ];

    metrics.observe(&reports);
    assert_eq!(metrics.briefs, 2);
    assert_eq!(metrics.tasks_with_violations, 1);
    assert_eq!(metrics.acted_while_blocked, 1);
    assert_eq!(metrics.evidence_below_spec, 1);
    assert_eq!(metrics.obligations_ignored, 0);
    assert_eq!(metrics.violation_rate_bp(), 5_000, "ORCH_GUIDANCE_METRICS");

    metrics.observe(&reports);
    assert_eq!(metrics.briefs, 4);
    assert_eq!(metrics.violation_rate_bp(), 5_000, "累计口径保持一致");
}
