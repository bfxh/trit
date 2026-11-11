use trit_gamescan::{
    CheckpointStore, ColliderShape, ColliderSnapshot, EntitySnapshot, InMemoryCheckpointStore,
    Mutation, MutationPlan, Mutator, RecoveryCoordinator, RecoveryStatus, ReplayWorld,
    ResourceSnapshot, RestoreLevel, Ternary, TransactionOutcome, WorldSnapshot,
    default_structural_rules, run_rules,
};

fn baseline() -> WorldSnapshot {
    let mut snapshot = WorldSnapshot::new("replay");
    snapshot.tick = 42;

    let mut root = EntitySnapshot::new(1);
    root.tags.insert("root".into());

    let mut child = EntitySnapshot::new(2).with_parent(1);
    child.transform.position = [2.0, 0.0, 0.0];
    child.tags.insert("dynamic".into());

    let mut sibling = EntitySnapshot::new(3).with_parent(1);
    sibling.transform.position = [4.0, 0.0, 0.0];

    snapshot.entities = vec![root, child, sibling];
    snapshot.resources = vec![
        ResourceSnapshot {
            key: "mesh/hero".into(),
            generation: 1,
        },
        ResourceSnapshot {
            key: "mat/hero".into(),
            generation: 1,
        },
    ];
    snapshot.colliders = vec![
        ColliderSnapshot::new(2, ColliderShape::Sphere { radius: 0.5 }),
        ColliderSnapshot::new(3, ColliderShape::Sphere { radius: 0.5 }),
    ];
    snapshot
}

fn fresh_coordinator() -> RecoveryCoordinator<InMemoryCheckpointStore> {
    RecoveryCoordinator::new(InMemoryCheckpointStore::new(), default_structural_rules())
}

fn plan_with(mutation: Mutation) -> MutationPlan {
    let mut plan = MutationPlan::new(0xC0FFEE);
    plan.push(mutation);
    plan
}

fn run_case(mutation: Mutation) -> (TransactionOutcome, u64, u64) {
    let mut world = ReplayWorld::new(baseline());
    let baseline_hash = world.snapshot().stable_hash();
    let mut coordinator = fresh_coordinator();
    coordinator.capture_pristine(&world).unwrap();

    let outcome = coordinator.run(&mut world, &plan_with(mutation)).unwrap();
    let restored_hash = world.snapshot().stable_hash();
    (outcome, baseline_hash, restored_hash)
}

fn assert_restored_at(outcome: &TransactionOutcome, expected: RestoreLevel) {
    match &outcome.recovery {
        RecoveryStatus::Restored { level, verify } => {
            assert_eq!(*level, expected);
            assert!(verify.passed(), "verification failed: {verify:?}");
        }
        RecoveryStatus::Quarantined { reason, .. } => {
            panic!("unexpected quarantine: {reason}");
        }
    }
}

fn has_rule(outcome: &TransactionOutcome, rule_id: &str) -> bool {
    outcome
        .report
        .findings()
        .any(|finding| finding.rule_id == rule_id)
}

#[test]
fn baseline_passes_all_default_rules() {
    let rules = default_structural_rules();
    let report = run_rules(&rules, &baseline());
    assert_eq!(
        report.trit(),
        Ternary::True,
        "unexpected findings: {:?}",
        report.findings().collect::<Vec<_>>()
    );
}

#[test]
fn nan_position_is_detected_and_recovered() {
    let (outcome, baseline_hash, restored_hash) = run_case(Mutation::nan_position(2));
    assert!(has_rule(&outcome, "physics.transform_finite"));
    assert_eq!(outcome.report.trit(), Ternary::False);
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(restored_hash, baseline_hash);
}

#[test]
fn zero_scale_is_detected() {
    let (outcome, baseline_hash, restored_hash) = run_case(Mutation::zero_scale(2));
    assert!(has_rule(&outcome, "physics.scale_nonzero_finite"));
    assert_eq!(outcome.report.trit(), Ternary::False);
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(restored_hash, baseline_hash);
}

#[test]
fn invalid_rotation_is_detected() {
    let (outcome, baseline_hash, restored_hash) = run_case(Mutation::invalid_rotation(2));
    assert!(has_rule(&outcome, "physics.rotation_normalized"));
    assert_eq!(outcome.report.trit(), Ternary::False);
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(restored_hash, baseline_hash);
}

#[test]
fn missing_parent_is_detected() {
    let (outcome, baseline_hash, restored_hash) = run_case(Mutation::dangling_parent(3, 999));
    assert!(has_rule(&outcome, "structure.parents_exist"));
    assert_eq!(outcome.report.trit(), Ternary::False);
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(restored_hash, baseline_hash);
}

#[test]
fn hierarchy_cycle_is_detected() {
    let (outcome, baseline_hash, restored_hash) = run_case(Mutation::SetParent {
        id: 1,
        parent: Some(2),
    });
    assert!(has_rule(&outcome, "structure.no_hierarchy_cycles"));
    assert_eq!(outcome.report.trit(), Ternary::False);
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(restored_hash, baseline_hash);
}

#[test]
fn duplicate_ids_fail_and_cycle_rule_is_unknown() {
    let (outcome, baseline_hash, restored_hash) =
        run_case(Mutation::SetEntityId { id: 3, new_id: 2 });
    assert!(has_rule(&outcome, "structure.no_duplicate_ids"));
    let cycle = outcome
        .report
        .outcomes
        .iter()
        .find(|outcome| outcome.rule_id == "structure.no_hierarchy_cycles")
        .expect("cycle outcome");
    assert_eq!(cycle.trit, Ternary::Unknown);
    assert_eq!(outcome.report.trit(), Ternary::False);
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(restored_hash, baseline_hash);
}

#[test]
fn penetration_is_detected_and_recovered() {
    let (outcome, baseline_hash, restored_hash) = run_case(Mutation::SetPosition {
        id: 2,
        position: [3.6, 0.0, 0.0],
    });
    assert!(has_rule(&outcome, "physics.penetration"));
    assert_eq!(outcome.report.trit(), Ternary::False);
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(restored_hash, baseline_hash);
}

#[test]
fn removing_root_creates_missing_parent_findings() {
    let (outcome, baseline_hash, restored_hash) = run_case(Mutation::RemoveEntity { id: 1 });
    assert!(has_rule(&outcome, "structure.parents_exist"));
    assert_eq!(outcome.report.trit(), Ternary::False);
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(restored_hash, baseline_hash);
}

#[test]
fn apply_error_still_rolls_back() {
    let mut world = ReplayWorld::new(baseline());
    let baseline_hash = world.snapshot().stable_hash();
    let mut coordinator = fresh_coordinator();
    coordinator.capture_pristine(&world).unwrap();

    let outcome = coordinator
        .run(&mut world, &plan_with(Mutation::RemoveEntity { id: 999 }))
        .unwrap();

    assert!(outcome.apply_error.is_some());
    assert_restored_at(&outcome, RestoreLevel::Checkpoint);
    assert_eq!(world.snapshot().stable_hash(), baseline_hash);
}

#[test]
fn checkpoint_restore_failure_falls_back_to_pristine() {
    let mut world = ReplayWorld::new(baseline());
    let baseline_hash = world.snapshot().stable_hash();
    let mut coordinator = fresh_coordinator();
    coordinator.capture_pristine(&world).unwrap();
    coordinator.store_mut().fail_next_restore();

    let outcome = coordinator
        .run(&mut world, &plan_with(Mutation::nan_position(2)))
        .unwrap();

    assert_restored_at(&outcome, RestoreLevel::Pristine);
    assert_eq!(world.snapshot().stable_hash(), baseline_hash);
}

#[test]
fn corrupt_verification_falls_back_to_pristine() {
    let mut world = ReplayWorld::new(baseline());
    let baseline_hash = world.snapshot().stable_hash();
    let mut coordinator = fresh_coordinator();
    coordinator.capture_pristine(&world).unwrap();
    coordinator.store_mut().corrupt_next_verify();

    let outcome = coordinator
        .run(&mut world, &plan_with(Mutation::nan_position(2)))
        .unwrap();

    assert_restored_at(&outcome, RestoreLevel::Pristine);
    assert_eq!(world.snapshot().stable_hash(), baseline_hash);
}

#[test]
fn all_restore_failures_quarantine() {
    let mut world = ReplayWorld::new(baseline());
    let mut coordinator = fresh_coordinator();
    coordinator.capture_pristine(&world).unwrap();
    coordinator.store_mut().fail_all_restores();

    let outcome = coordinator
        .run(&mut world, &plan_with(Mutation::nan_position(2)))
        .unwrap();

    match outcome.recovery {
        RecoveryStatus::Quarantined { reason, .. } => {
            assert!(!reason.is_empty());
        }
        RecoveryStatus::Restored { .. } => panic!("must not resume after failed recovery"),
    }
    assert_eq!(outcome.report.trit(), Ternary::False);
}

#[test]
fn same_plan_produces_same_report_signature() {
    let first = run_case(Mutation::nan_position(2)).0;
    let second = run_case(Mutation::nan_position(2)).0;
    assert_eq!(first.report.signature(), second.report.signature());
    assert_eq!(first.report.snapshot_hash, second.report.snapshot_hash);
}

#[test]
fn checkpoint_store_roundtrip_verifies() {
    let mut store = InMemoryCheckpointStore::new();
    let snapshot = baseline();
    let id = store.capture(&snapshot).unwrap();
    let restored = store.restore(id).unwrap();
    let verify = store.verify(id, &restored).unwrap();
    assert!(verify.passed(), "verification failed: {verify:?}");
    assert_eq!(restored.stable_hash(), snapshot.stable_hash());
}
