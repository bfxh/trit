use trit_orch::{
    BlockReason, ClaimSet, ClaimStore, Ledger, Limits, ObligationKind, Observation, Plan,
    Resources, Snapshot, Task, TaskSpec, Ternary,
};

fn task(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: Vec::new(),
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
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
        tokens: 1_000,
    }
}

#[test]
fn claimed_task_is_selected() {
    let tasks = vec![task("a")];
    let state = snapshot(&tasks);
    let mut store = ClaimStore::new();
    store
        .claim("alice", &["src/a.rs".into()], "edit", 100, 30)
        .unwrap();
    let mut claims = ClaimSet::new(&store, "alice", 101);
    claims.insert("a", vec!["src/a.rs".into()]);

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select_with_claims(&state, &limits(), &claims)
        .unwrap();
    assert_eq!(selection.selected, ["a"]);
}

#[test]
fn unclaimed_task_is_unknown_with_claim_obligation() {
    let tasks = vec![task("a")];
    let state = snapshot(&tasks);
    let store = ClaimStore::new();
    let mut claims = ClaimSet::new(&store, "alice", 101);
    claims.insert("a", vec!["src/a.rs".into()]);

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select_with_claims(&state, &limits(), &claims)
        .unwrap();
    let decision = &selection.decisions[0];
    assert!(selection.selected.is_empty());
    assert_eq!(decision.admission, Ternary::Unknown);
    assert!(decision.reasons.iter().any(|reason| matches!(
        reason,
        BlockReason::Claim {
            verdict: Ternary::Unknown
        }
    )));
    assert!(
        decision
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Claim)
    );
}

#[test]
fn other_owner_claim_denies_task() {
    let tasks = vec![task("a")];
    let state = snapshot(&tasks);
    let mut store = ClaimStore::new();
    store
        .claim("bob", &["src/a.rs".into()], "edit", 100, 30)
        .unwrap();
    let mut claims = ClaimSet::new(&store, "alice", 101);
    claims.insert("a", vec!["src/a.rs".into()]);

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select_with_claims(&state, &limits(), &claims)
        .unwrap();
    let decision = &selection.decisions[0];
    assert!(selection.selected.is_empty());
    assert_eq!(decision.admission, Ternary::False);
    assert!(decision.reasons.iter().any(|reason| matches!(
        reason,
        BlockReason::Claim {
            verdict: Ternary::False
        }
    )));
    assert!(decision.obligations.is_empty(), "明确拒绝不是补证");
}

#[test]
fn expired_claim_is_unknown_and_actionable() {
    let tasks = vec![task("a")];
    let state = snapshot(&tasks);
    let mut store = ClaimStore::new();
    store
        .claim("alice", &["src/a.rs".into()], "edit", 100, 10)
        .unwrap();
    let mut claims = ClaimSet::new(&store, "alice", 111);
    claims.insert("a", vec!["src/a.rs".into()]);

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select_with_claims(&state, &limits(), &claims)
        .unwrap();
    let decision = &selection.decisions[0];
    assert_eq!(decision.admission, Ternary::Unknown);
    assert!(
        decision
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Claim)
    );
}

#[test]
fn missing_task_paths_are_unknown() {
    let tasks = vec![task("a")];
    let state = snapshot(&tasks);
    let store = ClaimStore::new();
    let claims = ClaimSet::new(&store, "alice", 101);

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select_with_claims(&state, &limits(), &claims)
        .unwrap();
    let decision = &selection.decisions[0];
    assert_eq!(decision.admission, Ternary::Unknown);
    assert!(
        decision
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Claim)
    );
}

#[test]
fn empty_paths_require_no_claim() {
    let tasks = vec![task("a")];
    let state = snapshot(&tasks);
    let store = ClaimStore::new();
    let mut claims = ClaimSet::new(&store, "alice", 101);
    claims.insert("a", Vec::new());

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select_with_claims(&state, &limits(), &claims)
        .unwrap();
    assert_eq!(selection.selected, ["a"]);
}

#[test]
fn claim_gate_composes_with_effect_gate() {
    let tasks = vec![task("a")];
    let state = snapshot(&tasks);
    let store = ClaimStore::new();
    let mut claims = ClaimSet::new(&store, "alice", 101);
    claims.insert("a", vec!["src/a.rs".into()]);
    let ledger = Ledger::new();

    let selection = Plan::new(7, tasks)
        .unwrap()
        .select_with_effects_and_claims(&state, &limits(), &ledger, &claims)
        .unwrap();
    assert!(selection.selected.is_empty());
    assert!(
        selection.decisions[0]
            .obligations
            .iter()
            .any(|obligation| obligation.kind == ObligationKind::Claim)
    );
}
