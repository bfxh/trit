//! 阻塞原因 → 义务的路由。
//!
//! 结构性约束是「**不存在无义务的 Unknown**」：只要准入不是 True，就必须有一个具体的
//! 补证动作被记下来，否则"待补证"会变成"一直挂着没人管"。这里逐条把每个原因钉到它
//! 该产出的义务上，并且确认不是 Unknown 的原因**不**凭空造义务。

use trit_orch::{
    AdmissionObligation, BlockReason, Decision, Effect, ObligationKind, Resources, Selection,
    Ternary, TokenUsage, obligations_for,
};

fn kinds(obligations: &[AdmissionObligation]) -> Vec<&'static str> {
    obligations.iter().map(|o| o.kind.as_str()).collect()
}

fn only(reasons: &[BlockReason]) -> AdmissionObligation {
    let obligations = obligations_for("t", reasons);
    assert_eq!(obligations.len(), 1, "应当恰好一条义务：{obligations:?}");
    obligations[0].clone()
}

#[test]
fn undecided_dependency_is_deferred_not_measured() {
    // 「依赖要不要跑」还没定 → 先等（Defer）；「依赖跑没跑出结论」没定 → 去测（Measure）。
    // 两者混用会让执行侧去测一个根本不该跑的东西。
    let obligation = only(&[BlockReason::Dependency {
        task: "dep".into(),
        required: Ternary::Unknown,
        verdict: Ternary::Unknown,
    }]);
    assert_eq!(
        obligation.kind,
        ObligationKind::Defer,
        "ORCH_ROUTE_DEP_REQUIRED"
    );
    assert_eq!(obligation.subject, "dep");
}

#[test]
fn undecided_verdict_on_a_required_dependency_is_a_measure() {
    let obligation = only(&[BlockReason::Dependency {
        task: "dep".into(),
        required: Ternary::True,
        verdict: Ternary::Unknown,
    }]);
    assert_eq!(
        obligation.kind,
        ObligationKind::Measure,
        "ORCH_ROUTE_DEP_VERDICT"
    );
}

#[test]
fn a_decided_dependency_still_blocks_but_owes_nothing() {
    // required=True 且 verdict=False：这是一个明确的"依赖没做到"，属于拒绝，不是待补证。
    let obligations = obligations_for(
        "t",
        &[BlockReason::Dependency {
            task: "dep".into(),
            required: Ternary::True,
            verdict: Ternary::False,
        }],
    );
    assert_eq!(
        kinds(&obligations),
        vec!["defer"],
        "没有可补的证，只能是兜底"
    );
    assert_eq!(obligations[0].subject, "*", "ORCH_ROUTE_NO_BOGUS_MEASURE");
}

#[test]
fn unknown_authorization_escalates_to_a_human() {
    let obligation = only(&[BlockReason::Authorization {
        verdict: Ternary::Unknown,
    }]);
    assert_eq!(obligation.kind, ObligationKind::Escalate, "ORCH_ROUTE_AUTH");
    assert_eq!(obligation.subject, "authorization");
}

#[test]
fn a_known_authorization_verdict_owes_nothing() {
    for verdict in [Ternary::True, Ternary::False] {
        let obligations = obligations_for("t", &[BlockReason::Authorization { verdict }]);
        assert_eq!(obligations[0].kind, ObligationKind::Defer);
        assert_eq!(obligations[0].subject, "*", "ORCH_ROUTE_AUTH_KNOWN");
    }
}

#[test]
fn conflict_is_adjudicated_rather_than_measured() {
    // 支持和反对都有，再测一遍不会让结论变确定 —— 只能裁决。
    let obligation = only(&[BlockReason::Conflict { task: "dep".into() }]);
    assert_eq!(
        obligation.kind,
        ObligationKind::Adjudicate,
        "ORCH_ROUTE_CONFLICT"
    );
    assert_eq!(obligation.subject, "dep");
}

#[test]
fn stale_input_is_rebased() {
    let obligation = only(&[BlockReason::Freshness {
        verdict: Ternary::Unknown,
    }]);
    assert_eq!(
        obligation.kind,
        ObligationKind::Rebasis,
        "ORCH_ROUTE_FRESHNESS"
    );
}

#[test]
fn in_doubt_effects_are_reconciled_but_prepared_ones_are_not() {
    // 只有「可能已经发出去了」才需要对账。Prepared（还没发）对账是空转。
    for state in [Effect::Sending, Effect::Sent, Effect::Unknown] {
        let obligation = only(&[BlockReason::Effect { state }]);
        assert_eq!(
            obligation.kind,
            ObligationKind::Reconcile,
            "ORCH_ROUTE_EFFECT"
        );
        assert_eq!(obligation.subject, "t");
    }
    let obligations = obligations_for(
        "t",
        &[BlockReason::Effect {
            state: Effect::Prepared,
        }],
    );
    assert_eq!(obligations[0].subject, "*", "Prepared 不该被当成待对账");
}

#[test]
fn capacity_pressure_is_a_defer_with_a_stable_subject() {
    // 三种容量原因合成同一条义务，subject 固定为 resources，避免同批刷出三条一样的待办。
    let shared = only(&[
        BlockReason::Resources {
            requirements: Resources::new(1, 1),
            available: Resources::new(0, 0),
        },
        BlockReason::Workers {
            occupied: 4,
            limit: 4,
        },
        BlockReason::Tokens {
            required: 1000,
            available: 10,
        },
    ]);
    assert_eq!(shared.kind, ObligationKind::Defer, "ORCH_ROUTE_RESOURCES");
    assert_eq!(shared.subject, "resources");
}

#[test]
fn the_same_kind_and_subject_is_reported_once() {
    let duplicates = obligations_for(
        "t",
        &[
            BlockReason::Freshness {
                verdict: Ternary::Unknown,
            },
            BlockReason::Freshness {
                verdict: Ternary::False,
            },
        ],
    );
    assert_eq!(kinds(&duplicates), vec!["rebasis"], "ORCH_ROUTE_DEDUP");
}

#[test]
fn an_unclassifiable_reason_still_owes_something() {
    // 保证「不存在无义务的 Unknown」是结构性约束，而不是当前原因表的巧合：
    // 新增一种 BlockReason 而忘了路由时，这里会兜底出一条 Defer。
    let obligations = obligations_for(
        "t",
        &[BlockReason::Superseded {
            run_epoch: 1,
            task_epoch: 2,
        }],
    );
    assert_eq!(kinds(&obligations), vec!["defer"], "ORCH_ROUTE_FALLBACK");
    assert_eq!(obligations[0].subject, "*");
}

#[test]
fn every_block_reason_variant_maps_to_some_obligation() {
    // 穷举一遍原因表：任何一条都不允许返回空。这条会在新增变体时提醒去补路由。
    let reasons = vec![
        BlockReason::Dependency {
            task: "d".into(),
            required: Ternary::Unknown,
            verdict: Ternary::Unknown,
        },
        BlockReason::Authorization {
            verdict: Ternary::Unknown,
        },
        BlockReason::Freshness {
            verdict: Ternary::Unknown,
        },
        BlockReason::Effect {
            state: Effect::Unknown,
        },
        BlockReason::Conflict { task: "d".into() },
        BlockReason::ResourceCapacity {
            requirements: Resources::new(9, 9),
            capacity: Resources::new(1, 1),
        },
        BlockReason::Resources {
            requirements: Resources::new(1, 1),
            available: Resources::new(0, 0),
        },
        BlockReason::Workers {
            occupied: 1,
            limit: 1,
        },
        BlockReason::TokenCapacity {
            required: 999,
            limit: 10,
        },
        BlockReason::Tokens {
            required: 999,
            available: 10,
        },
        BlockReason::InFlight,
        BlockReason::Settled,
        BlockReason::Superseded {
            run_epoch: 1,
            task_epoch: 2,
        },
    ];
    let obligations = obligations_for("t", &reasons);
    assert!(!obligations.is_empty(), "ORCH_ROUTE_NON_EMPTY");
    assert!(
        obligations.iter().all(|o| !o.id.trim().is_empty()),
        "每条义务都必须有可复核的 id"
    );
}

#[test]
fn obligations_are_deduplicated_across_decisions_but_keep_order() {
    let obligation = |task: &str, subject: &str| {
        AdmissionObligation::new(ObligationKind::Measure, task, subject, "x")
    };
    let selection = Selection {
        revision: 1,
        selected: vec![],
        decisions: vec![
            Decision {
                task: "a".into(),
                admission: Ternary::Unknown,
                reasons: vec![],
                obligations: vec![obligation("a", "s1"), obligation("a", "s1")],
            },
            Decision {
                task: "b".into(),
                admission: Ternary::Unknown,
                reasons: vec![],
                obligations: vec![obligation("b", "s2"), obligation("a", "s1")],
            },
        ],
        observed_tokens: TokenUsage::default(),
        reserved_tokens: 0,
        reserved_resources: Resources::new(0, 0),
        occupied_workers: 0,
    };
    let collected = selection.obligations();
    let ids: Vec<&str> = collected.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["measure:a:s1", "measure:b:s2"],
        "ORCH_ROUTE_SELECTION_DEDUP"
    );
}

#[test]
fn a_blank_id_is_repaired_on_collection() {
    // 手工构造的义务可能没有 id；收集时必须补出来，否则日志里会出现无名待办。
    let mut obligation = AdmissionObligation::new(ObligationKind::Escalate, "t", "auth", "x");
    obligation.id = "   ".into();
    let selection = Selection {
        revision: 1,
        selected: vec![],
        decisions: vec![Decision {
            task: "t".into(),
            admission: Ternary::Unknown,
            reasons: vec![],
            obligations: vec![obligation],
        }],
        observed_tokens: TokenUsage::default(),
        reserved_tokens: 0,
        reserved_resources: Resources::new(0, 0),
        occupied_workers: 0,
    };
    let collected = selection.obligations();
    assert_eq!(collected[0].id, "escalate:t:auth", "ORCH_ROUTE_ENSURE_ID");
}
