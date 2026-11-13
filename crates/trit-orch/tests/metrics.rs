use std::collections::BTreeMap;

use trit_orch::{
    EffectSpec, InterventionPolicy, Ledger, Limits, Observation, OrchestrationMetrics, Plan,
    Resources, Snapshot, Task, TaskSpec, Ternary, TickInput, TokenUsage, tick,
};

static POLICY: InterventionPolicy = InterventionPolicy {
    quarantine_unsafe: true,
};

fn task(id: &str, effects: Vec<EffectSpec>, max_tokens: u64) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        max_tokens,
        effects,
        ..TaskSpec::default()
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
fn captures_concurrency_tokens_and_cache_hit_rate() {
    let tasks = vec![
        task("a", vec![], 100),
        task("b", vec![EffectSpec::new("b.write")], 1_000),
    ];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let mut observations = BTreeMap::new();
    for spec in &tasks {
        observations.insert(
            spec.task.id.clone(),
            Observation {
                authorization: Ternary::True,
                freshness: Ternary::True,
                usage: if spec.task.id == "b" {
                    TokenUsage {
                        fresh: 100,
                        cached: 300,
                        output: 50,
                    }
                } else {
                    TokenUsage::default()
                },
                ..Observation::default()
            },
        );
    }
    let snapshot = Snapshot {
        revision: 1,
        observations,
    };
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
    let metrics = OrchestrationMetrics::capture(&result, &snapshot, &limits());

    assert_eq!(metrics.tasks_total, 2);
    assert_eq!(metrics.tasks_selected, 1);
    assert_eq!(metrics.tasks_unknown, 1);
    assert_eq!(metrics.tasks_denied, 0);
    assert_eq!(metrics.tasks_in_flight, 0);
    assert_eq!(metrics.workers_limit, 4);
    assert_eq!(metrics.workers_occupied, 1);
    assert_eq!(metrics.concurrency_pressure_bp, 2_500);
    assert_eq!(metrics.selection_pressure_bp, 5_000);
    assert_eq!(metrics.tokens_fresh, 100);
    assert_eq!(metrics.tokens_cached, 300);
    assert_eq!(metrics.tokens_output, 50);
    assert_eq!(metrics.cache_hit_rate_bp, 7_500, "ORCH_METRICS_CACHE_HIT");
    assert_eq!(metrics.observed_tokens, 450);
    assert_eq!(metrics.granted_tokens, 100);
    assert_eq!(metrics.briefs, 1);
    assert_eq!(metrics.obligations, 1);
    assert_eq!(metrics.adherence_reports, 0);
    assert_eq!(metrics.violations, 0);
    assert_eq!(metrics.interventions, 0);
    assert_eq!(metrics.held, 0);
    assert_eq!(metrics.guidance_violation_rate_bp, 0);
    assert_eq!(metrics.blockers.len(), 1);
    assert_eq!(metrics.blockers["effect"], 1);

    let text = metrics.to_prometheus();
    assert!(text.contains("trit_orch_tasks_total 2"));
    assert!(text.contains("trit_orch_cache_hit_rate_bp 7500"));
    assert!(text.contains("trit_orch_workers_occupied 1"));
    assert!(text.contains("trit_orch_blocker_reasons{reason=\"effect\"} 1"));
    assert!(
        metrics
            .to_prometheus_with("custom")
            .contains("custom_tasks_total 2")
    );
}

#[test]
fn empty_tick_produces_zeroed_metrics() {
    let plan = Plan::new(1, vec![]).unwrap();
    let snapshot = Snapshot {
        revision: 1,
        observations: BTreeMap::new(),
    };
    let result = tick(TickInput {
        plan: &plan,
        snapshot: &snapshot,
        limits: &limits(),
        ledger: &Ledger::new(),
        claims: None,
        briefed: &[],
        trace: None,
        since_seq: 0,
        intervention: &POLICY,
    })
    .unwrap();

    let metrics = OrchestrationMetrics::capture(&result, &snapshot, &limits());
    assert_eq!(metrics.tasks_total, 0);
    assert_eq!(metrics.workers_limit, 4);
    assert_eq!(metrics.workers_occupied, 0);
    assert_eq!(metrics.cache_hit_rate_bp, 0);
    assert_eq!(metrics.concurrency_pressure_bp, 0);
    assert_eq!(metrics.selection_pressure_bp, 0);
    assert!(metrics.blockers.is_empty());
}
