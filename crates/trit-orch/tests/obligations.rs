use trit_orch::{
    AdmissionObligation, BlockReason, CheckReport, CheckResult, Decision, DeliveryReceipt,
    Dependency, Effect, HandoffPacket, Ledger, Limits, ObligationBudget, ObligationKind,
    ObligationPolicy, ObligationSkipReason, Observation, Phase, Plan, ReceiptState, Resources,
    Snapshot, Task, TaskSpec, Ternary, dispatch_ready, handoff_obligation, receipt_obligation,
    schedule_obligations,
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
        epoch: 0,
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
        workers: 4,
        capacity: Resources::new(100, 100),
        tokens: 10_000,
    }
}

fn passed() -> CheckReport {
    CheckReport {
        epoch: 0,
        checks: 1,
        result: CheckResult::Passed,
    }
}

/// 在途 / 已结算是状态，不是待补证的 Unknown；它们不应该被义务队列收编。
fn is_status_only(decision: &Decision) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| matches!(reason, BlockReason::InFlight | BlockReason::Settled))
}

#[test]
fn no_unknown_admission_without_obligation() {
    let mut impossible = task("impossible", 0, vec![]);
    impossible.requirements = Resources::new(101, 1); // 超过容量上限：False，不是 Unknown

    let tasks = vec![
        task("dep_pending", 0, vec![]),
        task("blocked", 0, vec![edge("dep_pending", Ternary::True)]),
        task("dep_ok", 0, vec![]),
        task("relevance", 0, vec![edge("dep_ok", Ternary::Unknown)]),
        task("noauth", 0, vec![]),
        task("stale", 0, vec![]),
        task("running", 0, vec![]),
        task("done", 0, vec![]),
        impossible,
    ];
    let mut state = snapshot(&tasks);

    let dep_ok = state.observations.get_mut("dep_ok").unwrap();
    dep_ok.phase = Phase::Settled;
    dep_ok.evidence = Some(passed());
    let done = state.observations.get_mut("done").unwrap();
    done.phase = Phase::Settled;
    done.evidence = Some(passed());
    state.observations.get_mut("noauth").unwrap().authorization = Ternary::Unknown;
    state.observations.get_mut("stale").unwrap().freshness = Ternary::False;
    state.observations.get_mut("running").unwrap().phase = Phase::InFlight;

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select(&state, &limits())
        .unwrap();

    let unknown: Vec<&Decision> = selection
        .decisions
        .iter()
        .filter(|decision| decision.admission == Ternary::Unknown && !is_status_only(decision))
        .collect();
    assert_eq!(
        unknown.len(),
        4,
        "场景必须覆盖四条 Unknown 路径：{unknown:#?}"
    );

    for decision in &unknown {
        assert!(
            !decision.obligations.is_empty(),
            "未知准入没有义务：{decision:#?}"
        );
        for obligation in &decision.obligations {
            assert_eq!(obligation.task, decision.task);
            assert!(!obligation.subject.trim().is_empty());
            assert!(!obligation.detail.trim().is_empty());
        }
    }

    let kinds = |name: &str| -> Vec<ObligationKind> {
        selection
            .decisions
            .iter()
            .find(|decision| decision.task == name)
            .unwrap()
            .obligations
            .iter()
            .map(|obligation| obligation.kind)
            .collect()
    };
    assert!(kinds("blocked").contains(&ObligationKind::Measure));
    assert!(kinds("relevance").contains(&ObligationKind::Defer));
    assert!(kinds("noauth").contains(&ObligationKind::Escalate));
    assert!(kinds("stale").contains(&ObligationKind::Rebasis));

    let blocked = selection
        .decisions
        .iter()
        .find(|decision| decision.task == "blocked")
        .unwrap();
    assert_eq!(blocked.obligations[0].subject, "dep_pending");

    for name in ["running", "done", "impossible"] {
        let decision = selection
            .decisions
            .iter()
            .find(|decision| decision.task == name)
            .unwrap();
        assert!(
            decision.obligations.is_empty(),
            "{name} 不该被义务队列收编：{decision:#?}"
        );
    }
}

fn ledger_in(state: Effect) -> Ledger {
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("pay", 0, "pay", 1).unwrap();
    match state {
        Effect::Prepared => {}
        Effect::Sending => {
            ledger.mark_sending(&attempt, 1).unwrap();
        }
        Effect::Sent => {
            ledger.mark_sending(&attempt, 1).unwrap();
            ledger.mark_sent(&attempt, 1).unwrap();
        }
        Effect::Unknown => {
            ledger.mark_sending(&attempt, 1).unwrap();
            ledger.mark_unknown(&attempt).unwrap();
        }
        Effect::Confirmed => {
            ledger.mark_sending(&attempt, 1).unwrap();
            ledger.record_result(&attempt, true).unwrap();
        }
        Effect::Refuted => {
            ledger.mark_sending(&attempt, 1).unwrap();
            ledger.record_result(&attempt, false).unwrap();
        }
    }
    ledger
}

#[test]
fn in_doubt_effects_require_reconcile_and_are_not_selected() {
    let tasks = vec![task("pay", 0, vec![]), task("other", 0, vec![])];

    for effect in [Effect::Sending, Effect::Sent, Effect::Unknown] {
        let mut ledger = ledger_in(effect);
        ledger.prepare("other", 0, "other", 1).unwrap();
        let selection = Plan::new(7, tasks.clone())
            .unwrap()
            .select_with_effects(&snapshot(&tasks), &limits(), &ledger)
            .unwrap();

        assert_eq!(selection.selected, vec!["other"], "{effect:?}");
        let pay = selection
            .decisions
            .iter()
            .find(|decision| decision.task == "pay")
            .unwrap();
        assert_eq!(pay.admission, Ternary::Unknown, "{effect:?}");
        assert!(
            pay.obligations
                .iter()
                .any(|obligation| obligation.kind == ObligationKind::Reconcile),
            "{effect:?}: {pay:#?}"
        );
        let ready = dispatch_ready(&selection, &tasks, &ledger).unwrap();
        assert_eq!(ready.len(), 1, "{effect:?}");
        assert_eq!(ready[0].operation, "other");
    }

    // Prepared 是唯一允许真正发出的状态。
    let mut ledger = ledger_in(Effect::Prepared);
    ledger.prepare("other", 0, "other", 1).unwrap();
    let selection = Plan::new(7, tasks.clone())
        .unwrap()
        .select_with_effects(&snapshot(&tasks), &limits(), &ledger)
        .unwrap();
    assert_eq!(selection.selected, vec!["pay", "other"]);
    assert_eq!(
        selection
            .decisions
            .iter()
            .find(|decision| decision.task == "pay")
            .unwrap()
            .admission,
        Ternary::True
    );

    // Confirmed 已发生：不允许重发，也不需要补证。
    let mut ledger = ledger_in(Effect::Confirmed);
    ledger.prepare("other", 0, "other", 1).unwrap();
    let state = snapshot(&tasks);
    let selection = Plan::new(7, tasks)
        .unwrap()
        .select_with_effects(&state, &limits(), &ledger)
        .unwrap();
    assert_eq!(selection.selected, vec!["other"]);
    let pay = selection
        .decisions
        .iter()
        .find(|decision| decision.task == "pay")
        .unwrap();
    assert_eq!(pay.admission, Ternary::False);
    assert!(pay.obligations.is_empty(), "{pay:#?}");
}

#[test]
fn conflicted_evidence_routes_to_adjudication_not_more_measurement() {
    let tasks = vec![
        task("dep", 0, vec![]),
        task("blocked", 0, vec![edge("dep", Ternary::True)]),
        task("optional", 0, vec![edge("dep", Ternary::False)]),
        task("maybe", 0, vec![edge("dep", Ternary::Unknown)]),
    ];
    let mut state = snapshot(&tasks);
    let dep = state.observations.get_mut("dep").unwrap();
    dep.phase = Phase::Settled;
    dep.evidence = Some(CheckReport {
        epoch: 0,
        checks: 2,
        result: CheckResult::Conflicted,
    });

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select(&state, &limits())
        .unwrap();
    let blocked = selection
        .decisions
        .iter()
        .find(|decision| decision.task == "blocked")
        .unwrap();
    assert_eq!(blocked.admission, Ternary::Unknown);
    assert!(
        blocked
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Adjudicate),
        "{blocked:#?}"
    );
    assert!(
        !blocked
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Measure),
        "冲突不能降级成补测：{blocked:#?}"
    );

    let optional = selection
        .decisions
        .iter()
        .find(|decision| decision.task == "optional")
        .unwrap();
    assert_eq!(
        optional.admission,
        Ternary::True,
        "可选依赖的证据冲突不应阻塞"
    );

    let maybe = selection
        .decisions
        .iter()
        .find(|decision| decision.task == "maybe")
        .unwrap();
    assert_eq!(maybe.admission, Ternary::Unknown);
    assert!(
        maybe
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Defer),
        "相关性未知应先决定需不需要：{maybe:#?}"
    );
    assert!(
        maybe
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Adjudicate),
        "冲突义务不能被相关性未知遮掉：{maybe:#?}"
    );
}

fn packet(expires_at: u64) -> HandoffPacket {
    HandoffPacket {
        id: "h1".into(),
        from: "alice".into(),
        to: "bob".into(),
        goal: "finish".into(),
        base_commit: "abc".into(),
        state_hash: "sha256:abc".into(),
        decisions: Vec::new(),
        open_questions: Vec::new(),
        claims: Vec::new(),
        evidence: Vec::new(),
        next_action: "run gates".into(),
        idempotency_key: "handoff:h1".into(),
        expires_at,
    }
}

#[test]
fn handoff_and_receipt_obligations_close_the_coordination_loop() {
    let valid = packet(200);
    assert!(handoff_obligation(&valid, "sha256:abc", 100).is_none());

    let mismatch =
        handoff_obligation(&packet(200), "sha256:other", 100).expect("状态哈希不一致必须产出义务");
    assert_eq!(mismatch.kind, ObligationKind::Handoff);
    assert_eq!(mismatch.subject, "h1");
    let expired =
        handoff_obligation(&packet(200), "sha256:abc", 201).expect("过期交接包必须产出义务");
    assert_eq!(expired.kind, ObligationKind::Handoff);

    let mut invalid = packet(200);
    invalid.goal.clear();
    let obligation =
        handoff_obligation(&invalid, "sha256:abc", 100).expect("结构不合法也必须给出可执行动作");
    assert_eq!(obligation.kind, ObligationKind::Handoff);

    let mut receipt = DeliveryReceipt::sent("r1", "k1").unwrap();
    assert_eq!(
        receipt_obligation(&receipt).unwrap().kind,
        ObligationKind::Handoff,
        "Sent 只证明邮局工作"
    );
    receipt.advance(ReceiptState::Delivered).unwrap();
    assert!(receipt_obligation(&receipt).is_some());
    receipt.advance(ReceiptState::Read).unwrap();
    assert!(receipt_obligation(&receipt).is_some(), "Read 不等于 Acted");
    receipt.advance(ReceiptState::Acted).unwrap();
    assert!(receipt_obligation(&receipt).is_none());

    let mut rejected = DeliveryReceipt::sent("r2", "k2").unwrap();
    rejected.reject("not mine").unwrap();
    assert_eq!(
        receipt_obligation(&rejected).unwrap().kind,
        ObligationKind::Handoff
    );
    let mut expired = DeliveryReceipt::sent("r3", "k3").unwrap();
    expired.expire().unwrap();
    assert_eq!(
        receipt_obligation(&expired).unwrap().kind,
        ObligationKind::Handoff
    );
}

#[test]
fn obligations_carry_cost_owner_priority_and_stable_id() {
    let escalate = AdmissionObligation::new(
        ObligationKind::Escalate,
        "task-a",
        "authorization",
        "授权无法判定",
    );
    assert_eq!(escalate.id, "escalate:task-a:authorization");
    assert_eq!(escalate.owner.as_deref(), Some("human"));
    assert!(escalate.cost.human);
    assert_eq!(escalate.cost.tokens, 0);
    assert_eq!(escalate.priority, 100);

    let reconcile = AdmissionObligation::new(ObligationKind::Reconcile, "pay", "pay", "对账");
    assert_eq!(reconcile.id, "reconcile:pay:pay");
    assert_eq!(reconcile.owner.as_deref(), Some("effect-owner"));
    assert_eq!(reconcile.cost.tokens, 200);
    assert_eq!(reconcile.priority, 90);

    let measure = AdmissionObligation::new(ObligationKind::Measure, "task-b", "dep", "补证");
    assert!(measure.owner.is_none());
    assert_eq!(measure.cost.tokens, 500);
    assert_eq!(measure.priority, 60);
}

fn at(
    kind: ObligationKind,
    task: &str,
    subject: &str,
    deadline: Option<u64>,
) -> AdmissionObligation {
    let obligation = AdmissionObligation::new(kind, task, subject, "test");
    match deadline {
        Some(deadline) => obligation.with_deadline(deadline),
        None => obligation,
    }
}

#[test]
fn scheduler_orders_by_deadline_then_priority_and_reports_overdue() {
    let late = at(ObligationKind::Measure, "late", "dep", Some(200));
    let early = at(ObligationKind::Reconcile, "early", "early", Some(100));
    let undated = at(ObligationKind::Escalate, "human", "authorization", None);
    let overdue = at(ObligationKind::Adjudicate, "overdue", "dep", Some(40));
    let batch = schedule_obligations(
        vec![late, undated, overdue, early],
        50,
        &ObligationBudget {
            tokens: 10_000,
            human: 1,
        },
    );
    let ids: Vec<&str> = batch.scheduled.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "adjudicate:overdue:dep",
            "reconcile:early:early",
            "measure:late:dep",
            "escalate:human:authorization"
        ]
    );
    assert_eq!(batch.overdue, ["adjudicate:overdue:dep"]);
    assert_eq!(batch.human_used, 1);
}

#[test]
fn scheduler_skips_budget_blocks_without_head_of_line_blocking() {
    let big = at(ObligationKind::Measure, "big", "dep", Some(100));
    let small = at(ObligationKind::Reconcile, "small", "small", Some(200));
    let batch = schedule_obligations(
        vec![big, small],
        50,
        &ObligationBudget {
            tokens: 300,
            human: 0,
        },
    );
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.scheduled[0].id, "reconcile:small:small");
    assert!(matches!(
        batch.skipped[0].reason,
        ObligationSkipReason::OverBudget {
            required: 500,
            available: 300
        }
    ));
    assert_eq!(batch.tokens_used, 200);
}

#[test]
fn scheduler_enforces_human_budget() {
    let human = at(
        ObligationKind::Escalate,
        "human",
        "authorization",
        Some(100),
    );
    let machine = at(ObligationKind::Measure, "machine", "dep", Some(200));
    let batch = schedule_obligations(
        vec![human, machine],
        50,
        &ObligationBudget {
            tokens: 10_000,
            human: 0,
        },
    );
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.scheduled[0].id, "measure:machine:dep");
    assert_eq!(batch.skipped[0].id, "escalate:human:authorization");
    assert_eq!(batch.skipped[0].reason, ObligationSkipReason::NoHuman);
}

#[test]
fn policy_fills_owner_and_deadline_without_overwriting_explicit_values() {
    let policy = ObligationPolicy {
        ttl: 10,
        default_owner: Some("agent-x".into()),
    };
    let assigned = policy.assign(
        vec![
            AdmissionObligation::new(ObligationKind::Measure, "a", "dep", ""),
            AdmissionObligation::new(ObligationKind::Escalate, "h", "authorization", ""),
            AdmissionObligation::new(ObligationKind::Measure, "b", "dep", "")
                .with_owner("bob")
                .with_deadline(999),
        ],
        100,
    );
    assert_eq!(assigned[0].owner.as_deref(), Some("agent-x"));
    assert_eq!(assigned[0].deadline, Some(110));
    assert_eq!(assigned[1].owner.as_deref(), Some("human"));
    assert_eq!(assigned[1].deadline, Some(110));
    assert_eq!(assigned[2].owner.as_deref(), Some("bob"));
    assert_eq!(assigned[2].deadline, Some(999));
}

#[test]
fn duplicate_obligations_are_scheduled_once() {
    let obligation = at(ObligationKind::Measure, "a", "dep", Some(100));
    let batch = schedule_obligations(
        vec![obligation.clone(), obligation],
        50,
        &ObligationBudget {
            tokens: 10_000,
            human: 0,
        },
    );
    assert_eq!(batch.scheduled.len(), 1);
    assert_eq!(batch.skipped.len(), 1);
    assert_eq!(batch.skipped[0].reason, ObligationSkipReason::Duplicate);
}
