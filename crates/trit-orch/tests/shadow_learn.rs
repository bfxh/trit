use std::collections::BTreeMap;

use trit_orch::{
    Attempt, Baseline, CriticalPath, Eager, Error, Ledger, Limits, LongestFirst, Observation,
    Phase, PolicyLearnDeferReason, PolicyLearning, Resources, ShadowLearner, ShadowPolicy,
    Snapshot, Task, TaskSpec, Ternary, TraceStep, evaluate_policies, learn_builtin_policies,
    learn_policy, replay,
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
        ..TaskSpec::default()
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

fn recommended(learning: PolicyLearning) -> trit_orch::PolicyRecommendation {
    match learning {
        PolicyLearning::Recommended(recommendation) => recommendation,
        other => panic!("expected a recommendation, got {other:?}"),
    }
}

fn deferred(learning: PolicyLearning) -> trit_orch::PolicyLearnDeferral {
    match learning {
        PolicyLearning::Deferred(deferral) => deferral,
        other => panic!("expected a deferral, got {other:?}"),
    }
}

#[test]
fn recommends_the_safe_policy_with_the_best_agreement() {
    let steps = vec![step(&["a"], &["a"])];
    let recommendation =
        recommended(learn_policy(&steps, &[&Baseline as &dyn ShadowPolicy, &Eager]).unwrap());
    assert_eq!(
        recommendation.chosen.policy(),
        "baseline",
        "ORCH_LEARN_SAFE_WINS"
    );
    assert_eq!(recommendation.chosen.agreement_bp(), 10_000);
    assert_eq!(
        recommendation.excluded_unsafe,
        ["eager"],
        "ORCH_LEARN_UNSAFE_EXCLUDED"
    );
}

#[test]
fn defers_when_every_candidate_is_unsafe() {
    let steps = vec![step(&["a"], &["a"])];
    let deferral = deferred(learn_policy(&steps, &[&Eager as &dyn ShadowPolicy]).unwrap());
    assert_eq!(
        deferral.reason,
        PolicyLearnDeferReason::NoSafeCandidate,
        "ORCH_LEARN_FAIL_CLOSED"
    );
    assert_eq!(deferral.evaluations.len(), 1);
    assert!(!deferral.evaluations[0].is_safe());
}

#[test]
fn recommendation_is_deterministic_under_candidate_order() {
    let steps = vec![step(&["a", "b", "c"], &[]), step(&["d"], &[])];
    let forward = learn_policy(
        &steps,
        &[&Baseline as &dyn ShadowPolicy, &LongestFirst, &CriticalPath],
    )
    .unwrap();
    let reversed = learn_policy(
        &steps,
        &[&CriticalPath as &dyn ShadowPolicy, &LongestFirst, &Baseline],
    )
    .unwrap();
    assert_eq!(forward, reversed, "ORCH_LEARN_DETERMINISTIC");
    assert_eq!(
        forward,
        learn_policy(
            &steps,
            &[&Baseline as &dyn ShadowPolicy, &LongestFirst, &CriticalPath],
        )
        .unwrap()
    );
    assert_eq!(recommended(forward).chosen.policy(), "baseline");
}

#[test]
fn ties_break_by_policy_name() {
    let steps = vec![step(&["a", "b", "c"], &[]), step(&["d"], &[])];
    let recommendation = recommended(
        learn_policy(&steps, &[&LongestFirst as &dyn ShadowPolicy, &CriticalPath]).unwrap(),
    );
    assert_eq!(
        recommendation.chosen.policy(),
        "critical_path",
        "ORCH_LEARN_TIE_BREAK"
    );
}

#[test]
fn defers_on_empty_corpus_and_empty_candidate_list() {
    let empty_corpus = deferred(learn_policy(&[], &[&Baseline as &dyn ShadowPolicy]).unwrap());
    assert_eq!(empty_corpus.reason, PolicyLearnDeferReason::EmptyCorpus);

    let corpus = [step(&["a"], &[])];
    let no_candidates = deferred(learn_policy(&corpus, &[]).unwrap());
    assert_eq!(
        no_candidates.reason,
        PolicyLearnDeferReason::NoCandidates,
        "ORCH_LEARN_NO_CANDIDATES"
    );
}

#[test]
fn unknown_builtin_policy_fails_closed() {
    let steps = vec![step(&["a"], &[])];
    assert_eq!(
        learn_builtin_policies(&steps, &["ghost"]),
        Err(Error::UnknownShadowPolicy {
            name: "ghost".into()
        }),
        "ORCH_LEARN_UNKNOWN_POLICY"
    );
    let recommendation =
        recommended(learn_builtin_policies(&steps, &["baseline", "eager"]).unwrap());
    assert_eq!(recommendation.chosen.policy(), "baseline");
}

#[test]
fn evaluation_matches_replay_reports() {
    let steps = vec![step(&["a", "b", "c"], &[]), step(&["d"], &[])];
    let evaluations =
        evaluate_policies(&steps, &[&Baseline as &dyn ShadowPolicy, &LongestFirst]).unwrap();
    assert_eq!(evaluations.len(), 2);
    assert_eq!(evaluations[0].policy(), "baseline");
    assert_eq!(evaluations[0].agreement_bp(), 10_000);

    let expected = replay(&LongestFirst, &steps).unwrap();
    assert_eq!(evaluations[1].report, expected);
    assert_eq!(evaluations[1].report.agreed_steps, 1);
    assert_eq!(
        evaluations[1].agreement_bp(),
        5_000,
        "ORCH_LEARN_AGREEMENT_BP"
    );
    assert!(evaluations[1].is_safe());
}

#[test]
fn online_learning_matches_replay_over_the_concatenated_corpus() {
    let first = vec![step(&["a"], &[])];
    let second = vec![step(&["a", "b"], &["b"])];
    let mut learner = ShadowLearner::new();
    learner
        .observe_builtin(&first, &["baseline", "eager"])
        .unwrap();
    learner
        .observe_builtin(&second, &["baseline", "eager"])
        .unwrap();

    let concatenated: Vec<TraceStep> = first.into_iter().chain(second).collect();
    let direct = replay(&Eager, &concatenated).unwrap();
    assert_eq!(
        learner.report("eager"),
        Some(&direct),
        "ORCH_LEARN_ONLINE_MERGE"
    );
    assert_eq!(learner.report("baseline").unwrap().agreed_steps, 2);
}

#[test]
fn safety_history_is_never_forgotten() {
    let mut learner = ShadowLearner::new();
    learner
        .observe_builtin(&[step(&["a"], &["a"])], &["baseline", "eager"])
        .unwrap();
    learner
        .observe_builtin(&[step(&["b"], &[])], &["baseline", "eager"])
        .unwrap();

    let recommendation = recommended(learner.learn());
    assert_eq!(recommendation.chosen.policy(), "baseline");
    assert_eq!(
        recommendation.excluded_unsafe,
        ["eager"],
        "ORCH_LEARN_HISTORY"
    );
}

#[test]
fn policy_set_changes_and_duplicates_fail_closed() {
    let mut learner = ShadowLearner::new();
    learner
        .observe_builtin(&[step(&["a"], &[])], &["baseline"])
        .unwrap();
    assert_eq!(
        learner.observe_builtin(&[step(&["a"], &[])], &["baseline", "eager"]),
        Err(Error::LearnPolicySetChanged {
            expected: vec!["baseline".into()],
            actual: vec!["baseline".into(), "eager".into()],
        })
    );
    assert_eq!(
        learner.observe(
            &[step(&["a"], &[])],
            &[&Baseline as &dyn ShadowPolicy, &Baseline],
        ),
        Err(Error::DuplicateShadowPolicy {
            name: "baseline".into()
        })
    );

    learner.reset();
    learner
        .observe_builtin(&[step(&["a"], &[])], &["baseline", "eager"])
        .unwrap();
    assert_eq!(recommended(learner.learn()).chosen.policy(), "baseline");
}

#[test]
fn empty_learner_defers_without_candidates() {
    let empty = deferred(ShadowLearner::new().learn());
    assert_eq!(empty.reason, PolicyLearnDeferReason::NoCandidates);

    let mut learner = ShadowLearner::new();
    learner.observe_builtin(&[], &["baseline"]).unwrap();
    assert_eq!(
        deferred(learner.learn()).reason,
        PolicyLearnDeferReason::EmptyCorpus
    );
}

#[test]
fn merging_learners_matches_replay_over_concatenated_corpora() {
    let first = vec![step(&["a"], &[])];
    let second = vec![step(&["a", "b"], &["b"])];
    let mut host_a = ShadowLearner::new();
    host_a
        .observe_builtin(&first, &["baseline", "eager"])
        .unwrap();
    let mut host_b = ShadowLearner::new();
    host_b
        .observe_builtin(&second, &["baseline", "eager"])
        .unwrap();

    host_a.merge(&host_b).unwrap();
    let concatenated: Vec<TraceStep> = first.into_iter().chain(second).collect();
    let direct = replay(&Eager, &concatenated).unwrap();
    assert_eq!(
        host_a.report("eager"),
        Some(&direct),
        "ORCH_LEARN_MERGE_REPLAY"
    );
    assert_eq!(host_a.report("baseline").unwrap().agreed_steps, 2);
    assert_eq!(recommended(host_a.learn()).chosen.policy(), "baseline");
}

#[test]
fn merge_rejects_policy_set_mismatch() {
    let corpus = [step(&["a"], &[])];
    let mut baseline_only = ShadowLearner::new();
    baseline_only
        .observe_builtin(&corpus, &["baseline"])
        .unwrap();
    let mut broader = ShadowLearner::new();
    broader
        .observe_builtin(&corpus, &["baseline", "eager"])
        .unwrap();

    assert_eq!(
        baseline_only.merge(&broader),
        Err(Error::LearnPolicySetChanged {
            expected: vec!["baseline".into()],
            actual: vec!["baseline".into(), "eager".into()],
        }),
        "ORCH_LEARN_MERGE_SET"
    );
}

#[test]
fn merging_an_empty_learner_adopts_the_other_side() {
    let corpus = [step(&["a"], &["a"])];
    let mut source = ShadowLearner::new();
    source
        .observe_builtin(&corpus, &["baseline", "eager"])
        .unwrap();

    let mut empty = ShadowLearner::new();
    empty.merge(&source).unwrap();
    assert_eq!(empty, source, "ORCH_LEARN_MERGE_EMPTY");

    let mut target = source.clone();
    target.merge(&ShadowLearner::new()).unwrap();
    assert_eq!(target, source);
}

#[test]
fn weighted_observation_matches_repeated_replay() {
    let batch = vec![step(&["a", "b"], &["b"])];
    let mut weighted = ShadowLearner::new();
    weighted
        .observe_builtin_weighted(&batch, &["baseline", "eager"], 3)
        .unwrap();

    let repeated: Vec<TraceStep> = batch.iter().chain(&batch).chain(&batch).cloned().collect();
    let direct = replay(&Eager, &repeated).unwrap();
    assert_eq!(
        weighted.report("eager"),
        Some(&direct),
        "ORCH_LEARN_WEIGHT_REPLAY"
    );
    assert_eq!(weighted.report("baseline").unwrap().steps, 3);
    assert_eq!(weighted.report("baseline").unwrap().agreed_steps, 3);
}

#[test]
fn weighted_merge_scales_the_other_host() {
    let first = vec![step(&["a"], &[])];
    let second = vec![step(&["a", "b"], &["b"])];
    let mut host_a = ShadowLearner::new();
    host_a
        .observe_builtin(&first, &["baseline", "eager"])
        .unwrap();
    let mut host_b = ShadowLearner::new();
    host_b
        .observe_builtin(&second, &["baseline", "eager"])
        .unwrap();

    host_a.merge_weighted(&host_b, 3).unwrap();
    let twice: Vec<TraceStep> = second.iter().chain(&second).cloned().collect();
    let concatenated: Vec<TraceStep> = first
        .into_iter()
        .chain(second.clone())
        .chain(second.clone())
        .chain(second)
        .collect();
    assert_eq!(
        host_a.report("eager"),
        Some(&replay(&Eager, &concatenated).unwrap()),
        "ORCH_LEARN_WEIGHT_MERGE"
    );

    let mut empty = ShadowLearner::new();
    empty.merge_weighted(&host_b, 2).unwrap();
    assert_eq!(
        empty.report("eager"),
        Some(&replay(&Eager, &twice).unwrap()),
        "空学习器按权重采纳应与重复回放一致"
    );
}

#[test]
fn zero_weight_is_a_no_op() {
    let corpus = [step(&["a"], &[])];
    let mut learner = ShadowLearner::new();
    learner
        .observe_builtin_weighted(&corpus, &["baseline"], 0)
        .unwrap();
    assert!(learner.is_empty(), "ORCH_LEARN_WEIGHT_ZERO");
    assert_eq!(
        deferred(learner.learn()).reason,
        PolicyLearnDeferReason::NoCandidates
    );

    let mut host = ShadowLearner::new();
    host.observe_builtin(&corpus, &["baseline"]).unwrap();
    let before = host.clone();
    host.merge_weighted(&before, 0).unwrap();
    assert_eq!(host, before, "ORCH_LEARN_WEIGHT_ZERO");
}

#[test]
fn windowed_learner_keeps_only_recent_steps() {
    let batches = [
        vec![step(&["a"], &[])],
        vec![step(&["b"], &["b"])],
        vec![step(&["c"], &[])],
    ];
    let mut learner = ShadowLearner::windowed(2).unwrap();
    for batch in &batches {
        learner
            .observe_builtin(batch, &["baseline", "eager"])
            .unwrap();
    }
    assert_eq!(learner.window(), Some(2));

    let retained: Vec<TraceStep> = batches[1].iter().chain(&batches[2]).cloned().collect();
    assert_eq!(
        learner.report("eager"),
        Some(&replay(&Eager, &retained).unwrap()),
        "ORCH_LEARN_WINDOW_TRIM"
    );
    assert_eq!(learner.report("baseline").unwrap().steps, 2);
    assert_eq!(recommended(learner.learn()).excluded_unsafe, ["eager"]);
}

#[test]
fn windowed_learning_can_rehabilitate_after_old_unsafe_ages_out() {
    let mut learner = ShadowLearner::windowed(1).unwrap();
    learner
        .observe_builtin(&[step(&["a"], &["a"])], &["baseline", "eager"])
        .unwrap();
    assert_eq!(recommended(learner.learn()).excluded_unsafe, ["eager"]);

    learner
        .observe_builtin(&[step(&["b"], &[])], &["baseline", "eager"])
        .unwrap();
    let recommendation = recommended(learner.learn());
    assert!(
        recommendation.excluded_unsafe.is_empty(),
        "ORCH_LEARN_WINDOW_REHABILITATE"
    );
    assert_eq!(
        learner.report("eager").unwrap().unsafe_extra,
        Vec::<String>::new()
    );
    assert_eq!(recommendation.chosen.policy(), "baseline");
}

#[test]
fn windowed_merge_keeps_the_latest_steps() {
    let host_a_batches = [vec![step(&["a"], &[])], vec![step(&["b"], &["b"])]];
    let host_b_batches = [vec![step(&["c"], &[])], vec![step(&["d"], &["d"])]];
    let mut host_a = ShadowLearner::windowed(2).unwrap();
    for batch in &host_a_batches {
        host_a
            .observe_builtin(batch, &["baseline", "eager"])
            .unwrap();
    }
    let mut host_b = ShadowLearner::windowed(2).unwrap();
    for batch in &host_b_batches {
        host_b
            .observe_builtin(batch, &["baseline", "eager"])
            .unwrap();
    }

    host_a.merge(&host_b).unwrap();
    let retained: Vec<TraceStep> = host_b_batches.iter().flatten().cloned().collect();
    assert_eq!(
        host_a.report("eager"),
        Some(&replay(&Eager, &retained).unwrap()),
        "ORCH_LEARN_WINDOW_MERGE"
    );
}

#[test]
fn window_config_mismatch_and_zero_window_fail_closed() {
    assert_eq!(
        ShadowLearner::windowed(0),
        Err(Error::LearnWindowZero),
        "ORCH_LEARN_WINDOW_ZERO"
    );

    let mut timed = ShadowLearner::windowed(2).unwrap();
    timed
        .observe_builtin(&[step(&["a"], &[])], &["baseline"])
        .unwrap();
    let mut cumulative = ShadowLearner::new();
    cumulative
        .observe_builtin(&[step(&["a"], &[])], &["baseline"])
        .unwrap();
    assert_eq!(
        timed.merge(&cumulative),
        Err(Error::LearnWindowMismatch {
            expected: Some(2),
            actual: None
        }),
        "ORCH_LEARN_WINDOW_MISMATCH"
    );
}
