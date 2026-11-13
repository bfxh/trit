use trit_orch::{
    AdmissionObligation, ClaimSet, ClaimStore, Dependency, Limits, ObligationKind,
    ObligationPlanDeferReason, ObligationTaskPolicy, Observation, Phase, Plan, Resources, Snapshot,
    Task, TaskSpec, Ternary, plan_patch_for_obligations, plan_patch_for_obligations_with_claims,
};

fn task(id: &str, deps: Vec<Dependency>) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps,
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
        ..TaskSpec::default()
    }
}

fn ready(tasks: &[TaskSpec], revision: u64) -> Snapshot {
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

fn obligation(kind: ObligationKind, target: &str, subject: &str) -> AdmissionObligation {
    AdmissionObligation::new(kind, target, subject, "test")
}

#[test]
fn measure_generates_task_and_rewires_pending_target() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let snapshot = ready(&tasks, 7);
    let result = plan_patch_for_obligations(
        &plan,
        &snapshot,
        [obligation(ObligationKind::Measure, "login", "auth")],
        &ObligationTaskPolicy::default(),
    );

    assert_eq!(result.patch.base_revision, 7);
    assert_eq!(result.patch.add.len(), 1);
    let generated = &result.patch.add[0];
    assert_eq!(generated.task.id, "ob:measure:login:auth");
    assert_eq!(generated.max_tokens, 500);
    assert_eq!(result.patch.replace.len(), 1);
    assert!(
        result.patch.replace[0]
            .task
            .deps
            .iter()
            .any(|dep| dep.task == generated.task.id && dep.required == Ternary::True)
    );
    assert_eq!(result.links.len(), 1);
    assert_eq!(result.links[0].generated_task, generated.task.id);

    let next = plan.patch(&result.patch, &snapshot).unwrap();
    assert_eq!(next.revision(), 8);
    assert_eq!(next.task("login").unwrap().epoch, 1);
    assert!(next.task(&generated.task.id).is_some());

    let after_tasks = vec![
        task("login", Vec::new()),
        next.task(&generated.task.id).unwrap().clone(),
    ];
    let after = ready(&after_tasks, 8);
    let selection = next.select(&after, &limits()).unwrap();
    assert!(selection.selected.contains(&generated.task.id));
    let login = selection
        .decisions
        .iter()
        .find(|decision| decision.task == "login")
        .unwrap();
    assert_eq!(login.admission, Ternary::Unknown);
}

#[test]
fn inflight_target_is_deferred() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut snapshot = ready(&tasks, 7);
    snapshot.observations.get_mut("login").unwrap().phase = Phase::InFlight;

    let result = plan_patch_for_obligations(
        &plan,
        &snapshot,
        [obligation(ObligationKind::Measure, "login", "auth")],
        &ObligationTaskPolicy::default(),
    );
    assert!(result.patch.add.is_empty());
    assert!(result.patch.replace.is_empty());
    assert_eq!(result.deferred.len(), 1);
    assert_eq!(
        result.deferred[0].reason,
        ObligationPlanDeferReason::TargetInFlight
    );
}

#[test]
fn remaining_kinds_generate_tasks() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let snapshot = ready(&tasks, 7);
    let result = plan_patch_for_obligations(
        &plan,
        &snapshot,
        [
            obligation(ObligationKind::Reconcile, "login", "pay"),
            obligation(ObligationKind::Escalate, "login", "auth"),
            obligation(ObligationKind::Handoff, "login", "h1"),
            obligation(ObligationKind::Defer, "login", "rate"),
        ],
        &ObligationTaskPolicy::default(),
    );
    assert!(result.deferred.is_empty(), "ORCH_OBLIGATION_ALL_KINDS");
    assert_eq!(result.patch.add.len(), 4);
    assert_eq!(result.patch.replace.len(), 1);
    assert_eq!(result.patch.replace[0].task.deps.len(), 4);
    assert!(
        result
            .patch
            .add
            .iter()
            .all(|spec| { spec.resolves == ["login"] && spec.produces.min_assertions == 1 })
    );

    let next = plan.patch(&result.patch, &snapshot).unwrap();
    let after = ready(next.tasks(), 8);
    assert_eq!(
        next.select(&after, &limits()).unwrap().selected.len(),
        4,
        "ORCH_OBLIGATION_ALL_KINDS_SELECTABLE"
    );
}

#[test]
fn generated_tasks_carry_native_resolves_and_produces() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let snapshot = ready(&tasks, 7);
    let policy = ObligationTaskPolicy {
        produces: trit_orch::EvidenceSpec { min_assertions: 2 },
        ..ObligationTaskPolicy::default()
    };

    let result = plan_patch_for_obligations(
        &plan,
        &snapshot,
        [obligation(ObligationKind::Measure, "login", "auth")],
        &policy,
    );
    let generated = &result.patch.add[0];
    assert_eq!(
        generated.resolves,
        ["login"],
        "ORCH_OBLIGATION_NATIVE_RESOLVES"
    );
    assert_eq!(
        generated.produces.min_assertions, 2,
        "ORCH_OBLIGATION_NATIVE_PRODUCES"
    );

    let next = plan.patch(&result.patch, &snapshot).unwrap();
    let resolvers: Vec<&str> = next
        .resolvers_of("login")
        .iter()
        .map(|spec| spec.task.id.as_str())
        .collect();
    assert_eq!(
        resolvers,
        ["ob:measure:login:auth"],
        "ORCH_OBLIGATION_RESOLVERS"
    );
    assert!(next.resolvers_of("ghost").is_empty());
}

#[test]
fn settled_or_missing_target_is_deferred() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let mut snapshot = ready(&tasks, 7);
    snapshot.observations.get_mut("login").unwrap().phase = Phase::Settled;

    let settled = plan_patch_for_obligations(
        &plan,
        &snapshot,
        [obligation(ObligationKind::Measure, "login", "auth")],
        &ObligationTaskPolicy::default(),
    );
    assert_eq!(
        settled.deferred[0].reason,
        ObligationPlanDeferReason::TargetNotPending
    );

    let missing = plan_patch_for_obligations(
        &plan,
        &ready(&tasks, 7),
        [obligation(ObligationKind::Measure, "ghost", "auth")],
        &ObligationTaskPolicy::default(),
    );
    assert_eq!(
        missing.deferred[0].reason,
        ObligationPlanDeferReason::TargetMissing
    );
}

#[test]
fn multiple_obligations_share_one_replacement() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let snapshot = ready(&tasks, 7);
    let result = plan_patch_for_obligations(
        &plan,
        &snapshot,
        [
            obligation(ObligationKind::Measure, "login", "auth"),
            obligation(ObligationKind::Rebasis, "login", "session"),
        ],
        &ObligationTaskPolicy::default(),
    );
    assert_eq!(result.patch.add.len(), 2);
    assert_eq!(result.patch.replace.len(), 1);
    assert_eq!(result.patch.replace[0].task.deps.len(), 2);
}

#[test]
fn existing_generated_task_is_deferred() {
    let generated = task("ob:measure:login:auth", Vec::new());
    let tasks = vec![task("login", Vec::new()), generated];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let result = plan_patch_for_obligations(
        &plan,
        &ready(&tasks, 7),
        [obligation(ObligationKind::Measure, "login", "auth")],
        &ObligationTaskPolicy::default(),
    );
    assert!(result.patch.add.is_empty());
    assert_eq!(
        result.deferred[0].reason,
        ObligationPlanDeferReason::TaskExists
    );
}

#[test]
fn resource_requirements_follow_kind_overrides() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let snapshot = ready(&tasks, 7);
    let mut policy = ObligationTaskPolicy::default();
    policy
        .requirements_by_kind
        .insert(ObligationKind::Adjudicate, Resources::new(2, 3));

    let result = plan_patch_for_obligations(
        &plan,
        &snapshot,
        [
            obligation(ObligationKind::Measure, "login", "auth"),
            obligation(ObligationKind::Adjudicate, "login", "session"),
        ],
        &policy,
    );
    let measure = result
        .patch
        .add
        .iter()
        .find(|spec| spec.task.id == "ob:measure:login:auth")
        .unwrap();
    let adjudicate = result
        .patch
        .add
        .iter()
        .find(|spec| spec.task.id == "ob:adjudicate:login:session")
        .unwrap();
    assert_eq!(
        measure.requirements,
        Resources::new(1, 1),
        "ORCH_OBLIGATION_RESOURCE_DEFAULT"
    );
    assert_eq!(
        adjudicate.requirements,
        Resources::new(2, 3),
        "ORCH_OBLIGATION_RESOURCE_KIND"
    );
}

#[test]
fn claim_obligation_maps_target_paths() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let snapshot = ready(&tasks, 7);
    let store = ClaimStore::new();
    let claims = ClaimSet::new(&store, "worker", 100).with_task(
        "login",
        vec![
            "src/login.rs".into(),
            "src/auth.rs".into(),
            "src/login.rs".into(),
        ],
    );

    let result = plan_patch_for_obligations_with_claims(
        &plan,
        &snapshot,
        &claims,
        [obligation(ObligationKind::Claim, "login", "claim")],
        &ObligationTaskPolicy::default(),
    );
    assert_eq!(result.patch.add.len(), 1);
    assert_eq!(result.patch.add[0].task.id, "ob:claim:login:claim");
    assert_eq!(
        result.links[0].paths,
        ["src/auth.rs", "src/login.rs"],
        "ORCH_OBLIGATION_CLAIM_PATHS"
    );
    assert_eq!(result.patch.replace.len(), 1);
    assert!(
        result.patch.replace[0]
            .task
            .deps
            .iter()
            .any(|dep| dep.task == "ob:claim:login:claim")
    );
}

#[test]
fn non_claim_obligations_still_carry_declared_paths() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let snapshot = ready(&tasks, 7);
    let store = ClaimStore::new();
    let claims =
        ClaimSet::new(&store, "worker", 100).with_task("login", vec!["src/auth.rs".into()]);

    let result = plan_patch_for_obligations_with_claims(
        &plan,
        &snapshot,
        &claims,
        [obligation(ObligationKind::Measure, "login", "auth")],
        &ObligationTaskPolicy::default(),
    );
    assert_eq!(result.links[0].paths, ["src/auth.rs"]);
}

#[test]
fn claim_without_declared_paths_is_deferred() {
    let tasks = vec![task("login", Vec::new())];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let snapshot = ready(&tasks, 7);

    let without_claims = plan_patch_for_obligations(
        &plan,
        &snapshot,
        [obligation(ObligationKind::Claim, "login", "claim")],
        &ObligationTaskPolicy::default(),
    );
    assert_eq!(
        without_claims.deferred[0].reason,
        ObligationPlanDeferReason::NoClaimPaths,
        "ORCH_OBLIGATION_CLAIM_NO_PATHS"
    );

    let store = ClaimStore::new();
    let empty = ClaimSet::new(&store, "worker", 100).with_task("login", Vec::new());
    let with_empty_paths = plan_patch_for_obligations_with_claims(
        &plan,
        &snapshot,
        &empty,
        [obligation(ObligationKind::Claim, "login", "claim")],
        &ObligationTaskPolicy::default(),
    );
    assert_eq!(
        with_empty_paths.deferred[0].reason,
        ObligationPlanDeferReason::NoClaimPaths
    );
}
