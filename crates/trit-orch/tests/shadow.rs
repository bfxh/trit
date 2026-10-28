use std::collections::BTreeMap;

use trit_orch::{
    Attempt, Baseline, Eager, Effect, Ledger, Limits, LongestFirst, Observation, Phase, Reconcile,
    Resources, ShadowPolicy, Snapshot, Task, TaskSpec, Ternary, TraceStep, replay,
};

fn task(id: &str, duration: u32) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration,
            deps: vec![],
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
    }
}

fn step(ids: &[&str], in_doubt: &[&str]) -> TraceStep {
    let tasks: Vec<TaskSpec> = ids.iter().map(|id| task(id, 1)).collect();
    let mut observations = BTreeMap::new();
    for id in ids {
        let phase = if in_doubt.contains(id) {
            Phase::InFlight
        } else {
            Phase::Pending
        };
        observations.insert(
            (*id).to_string(),
            Observation {
                phase,
                authorization: Ternary::True,
                freshness: Ternary::True,
                ..Observation::default()
            },
        );
    }
    let mut ledger = Ledger::new();
    for id in in_doubt {
        let attempt: Attempt = ledger.prepare("t", 0, id, 1).unwrap();
        ledger.mark_sending(&attempt, 1).unwrap();
    }
    TraceStep {
        revision: 1,
        tasks,
        snapshot: Snapshot {
            revision: 1,
            observations,
        },
        limits: Limits {
            workers: 4,
            capacity: Resources::new(4, 4),
            tokens: 10_000,
        },
        ledger,
    }
}

#[test]
fn baseline_agrees_with_the_kernel_on_every_step() {
    let steps = vec![step(&["a", "b"], &[]), step(&["c"], &[])];
    let report = replay(&Baseline, &steps).unwrap();
    assert_eq!(report.steps, 2);
    assert_eq!(report.agreed_steps, 2, "ORCH_SHADOW_BASELINE");
    assert!(report.rejected.is_empty());
    assert!(report.is_safe());
}

#[test]
fn order_only_proposals_do_not_change_the_dispatched_set() {
    let steps = vec![step(&["a", "b", "c"], &[])];
    let report = replay(&LongestFirst, &steps).unwrap();
    assert_eq!(report.rejected, Vec::<String>::new());
    assert_eq!(report.skipped, Vec::<String>::new());
    assert_eq!(report.order_mismatches, 1, "ORCH_SHADOW_ORDER");
    assert!(report.is_safe(), "顺序建议只应记差异，不算危险");
    assert_eq!(report.agreed_steps, 0);
}

#[test]
fn eager_with_nothing_extra_stays_safe() {
    let steps = vec![step(&["a"], &[])];
    let report = replay(&Eager, &steps).unwrap();
    assert!(report.rejected.is_empty());
    assert!(report.is_safe());
    assert_eq!(report.agreed_steps, 1);
}

#[test]
fn proposing_to_resend_an_in_doubt_effect_is_flagged_unsafe() {
    // a 在途：内核不会选它；eager 把它当“额外派发”提出来 → 必须判危险。
    let report = replay(&Eager, &[step(&["a"], &["a"])]).unwrap();
    assert_eq!(report.rejected, vec!["0:a"]);
    assert_eq!(report.unsafe_extra, vec!["0:a"], "ORCH_SHADOW_UNSAFE");
    assert!(!report.is_safe());
}

#[test]
fn reconciled_effect_is_still_unsafe_to_resend() {
    let mut trace = step(&["a"], &[]);
    let attempt = trace.ledger.prepare("t", 0, "a", 1).unwrap();
    trace.ledger.mark_sending(&attempt, 1).unwrap();
    trace.ledger.mark_unknown(&attempt).unwrap();
    trace
        .ledger
        .reconcile(&attempt, Reconcile::Present)
        .unwrap();
    assert_eq!(trace.ledger.state("a"), Some(Effect::Confirmed));
    trace.snapshot.observations.get_mut("a").unwrap().phase = Phase::Settled;
    let report = replay(&Eager, &[trace]).unwrap();
    assert_eq!(report.unsafe_extra, vec!["0:a"], "ORCH_SHADOW_UNSAFE");
    assert!(!report.is_safe());
}

#[test]
fn replay_is_deterministic_and_never_mutates_inputs() {
    let steps = vec![step(&["a", "b"], &[])];
    let before = steps.clone();
    let first = replay(&LongestFirst, &steps).unwrap();
    let second = replay(&LongestFirst, &steps).unwrap();
    assert_eq!(first, second, "ORCH_SHADOW_DETERMINISTIC");
    assert_eq!(steps, before, "回放不得改输入");
}

#[test]
fn policy_names_are_stable() {
    assert_eq!(Baseline.name(), "baseline");
    assert_eq!(LongestFirst.name(), "longest_first");
    assert_eq!(Eager.name(), "eager");
}
