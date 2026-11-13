use trit_orch::{
    AdmissionObligation, CheckReport, CheckResult, EvidenceRails, EvidenceVerdict,
    EvidenceWritebackDeferReason, Limits, ObligationKind, ObligationTaskPolicy, Observation, Phase,
    Plan, Resources, Snapshot, Task, TaskSpec, Ternary, plan_patch_for_obligations,
    writeback_obligation_evidence,
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

fn report(result: CheckResult) -> CheckReport {
    CheckReport {
        epoch: 0,
        checks: 1,
        result,
    }
}

/// 用真实 PlanPatch 切片生成：link、patch 后的 plan、以及对应 snapshot。
fn patched() -> (
    Plan,
    Snapshot,
    String,
    String,
    Vec<trit_orch::ObligationTaskLink>,
) {
    patched_with(&ObligationTaskPolicy::default())
}

fn patched_with(
    policy: &ObligationTaskPolicy,
) -> (
    Plan,
    Snapshot,
    String,
    String,
    Vec<trit_orch::ObligationTaskLink>,
) {
    let tasks = vec![task("login")];
    let plan = Plan::new(7, tasks.clone()).unwrap();
    let before = snapshot(&tasks, 7);
    let obligation = AdmissionObligation::new(ObligationKind::Measure, "login", "auth", "test");
    let result = plan_patch_for_obligations(&plan, &before, [obligation], policy);
    let generated = result.patch.add[0].task.id.clone();
    let next = plan.patch(&result.patch, &before).unwrap();
    let after_tasks = vec![task("login"), result.patch.add[0].clone()];
    let after = snapshot(&after_tasks, 8);
    assert_eq!(
        next.select(&after, &limits()).unwrap().selected,
        std::slice::from_ref(&generated)
    );
    (next, after, "login".into(), generated, result.links)
}

#[test]
fn writes_generated_evidence_and_rebases_to_target_epoch() {
    let (plan, mut snapshot, target, generated, links) = patched();
    let observation = snapshot.observations.get_mut(&generated).unwrap();
    observation.phase = Phase::Settled;
    observation.evidence = Some(report(CheckResult::Passed));

    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(written.written, 1);
    assert_eq!(written.skipped, 0);

    let target_observation = snapshot.observations.get(&target).unwrap();
    assert_eq!(target_observation.evidence_rails.support.len(), 1);
    let item = &target_observation.evidence_rails.support[0];
    assert_eq!(item.report.epoch, 1, "回写必须 rebase 到目标 epoch");
    assert!(item.source.contains(&generated));
    assert_eq!(target_observation.judgment(1), EvidenceVerdict::Proved);

    // 重复回写必须幂等。
    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(written.written, 0);
    assert_eq!(written.skipped, 1);
    assert_eq!(
        snapshot
            .observations
            .get(&target)
            .unwrap()
            .evidence_rails
            .support
            .len(),
        1
    );
}

#[test]
fn writes_multi_source_rails_as_conflict() {
    let (plan, mut snapshot, target, generated, links) = patched();
    let observation = snapshot.observations.get_mut(&generated).unwrap();
    observation.phase = Phase::Settled;
    let mut rails = EvidenceRails::default();
    rails.push("alice", report(CheckResult::Passed));
    rails.push("bob", report(CheckResult::Refuted));
    observation.evidence_rails = rails;

    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(written.written, 2);
    let target_observation = snapshot.observations.get(&target).unwrap();
    assert_eq!(target_observation.evidence_rails.support.len(), 1);
    assert_eq!(target_observation.evidence_rails.counter.len(), 1);
    assert_eq!(target_observation.judgment(1), EvidenceVerdict::Conflicted);
}

#[test]
fn defers_until_generated_task_is_settled_and_has_usable_evidence() {
    let (plan, mut snapshot, target, generated, links) = patched();

    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(written.written, 0);
    assert_eq!(
        written.deferred[0].reason,
        EvidenceWritebackDeferReason::GeneratedNotSettled
    );
    assert!(
        snapshot
            .observations
            .get(&target)
            .unwrap()
            .evidence_rails
            .is_empty()
    );

    let observation = snapshot.observations.get_mut(&generated).unwrap();
    observation.phase = Phase::Settled;
    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(
        written.deferred[0].reason,
        EvidenceWritebackDeferReason::GeneratedNoEvidence
    );

    snapshot.observations.get_mut(&generated).unwrap().evidence = Some(CheckReport {
        epoch: 99,
        checks: 1,
        result: CheckResult::Passed,
    });
    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(
        written.deferred[0].reason,
        EvidenceWritebackDeferReason::GeneratedEpochMismatch
    );
}

#[test]
fn defers_when_link_endpoints_are_missing() {
    let (plan, mut snapshot, target, generated, links) = patched();

    let without_generated = Plan::new(7, vec![task(&target)]).unwrap();
    let written = writeback_obligation_evidence(&without_generated, &mut snapshot.clone(), &links);
    assert_eq!(
        written.deferred[0].reason,
        EvidenceWritebackDeferReason::GeneratedMissing
    );

    let without_target = Plan::new(7, vec![task(&generated)]).unwrap();
    let written = writeback_obligation_evidence(&without_target, &mut snapshot.clone(), &links);
    assert_eq!(
        written.deferred[0].reason,
        EvidenceWritebackDeferReason::TargetMissing
    );

    let observation = snapshot.observations.get_mut(&generated).unwrap();
    observation.phase = Phase::Settled;
    observation.evidence = Some(report(CheckResult::Passed));
    snapshot.observations.remove(&target);
    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(
        written.deferred[0].reason,
        EvidenceWritebackDeferReason::TargetObservationMissing
    );
}

#[test]
fn writeback_enforces_the_declared_minimum_assertions() {
    let policy = ObligationTaskPolicy {
        produces: trit_orch::EvidenceSpec { min_assertions: 2 },
        ..ObligationTaskPolicy::default()
    };
    let (plan, mut snapshot, target, generated, links) = patched_with(&policy);
    let observation = snapshot.observations.get_mut(&generated).unwrap();
    observation.phase = Phase::Settled;
    observation.evidence = Some(report(CheckResult::Passed));

    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(written.written, 0);
    assert_eq!(
        written.deferred[0].reason,
        EvidenceWritebackDeferReason::InsufficientEvidence,
        "ORCH_WRITEBACK_MIN_ASSERTIONS"
    );
    assert!(
        snapshot
            .observations
            .get(&target)
            .unwrap()
            .evidence_rails
            .is_empty()
    );

    snapshot.observations.get_mut(&generated).unwrap().evidence = Some(CheckReport {
        epoch: 0,
        checks: 2,
        result: CheckResult::Passed,
    });
    let written = writeback_obligation_evidence(&plan, &mut snapshot, &links);
    assert_eq!(written.written, 1);
    assert_eq!(
        snapshot.observations.get(&target).unwrap().judgment(1),
        EvidenceVerdict::Proved
    );
}
