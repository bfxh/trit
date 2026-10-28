use trit_orch::{
    CheckReport, CheckResult, Observation, Phase, Ternary, TokenUsage, admit, dependency_gate,
};

#[test]
fn dependency_truth_table_does_not_infer_unknown_requirement() {
    use Ternary::{False as F, True as T, Unknown as U};
    let values = [F, U, T];
    let expected = [[T, T, T], [U, U, U], [F, U, T]];
    for (i, requirement) in values.into_iter().enumerate() {
        for (j, evidence) in values.into_iter().enumerate() {
            assert_eq!(
                dependency_gate(requirement, evidence),
                expected[i][j],
                "ORCH_DEPENDENCY_TABLE: {requirement:?}/{evidence:?}"
            );
        }
    }
}

#[test]
fn admission_matches_explicit_binary_unknown_oracle_for_all_81_inputs() {
    use Ternary::{False as F, True as T, Unknown as U};
    for dependencies in [F, U, T] {
        for authorization in [F, U, T] {
            for resources in [F, U, T] {
                for freshness in [F, U, T] {
                    let inputs = [dependencies, authorization, resources, freshness];
                    let denied = inputs.contains(&F);
                    let missing = inputs.contains(&U);
                    let expected = match (denied, missing) {
                        (true, _) => F,
                        (false, true) => U,
                        (false, false) => T,
                    };
                    assert_eq!(
                        admit(dependencies, authorization, resources, freshness),
                        expected,
                        "ORCH_ADMISSION_TABLE: {inputs:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn default_observation_is_unproven_not_false_or_authorized() {
    let observation = Observation::default();
    assert_eq!(observation.phase, Phase::Pending);
    assert_eq!(observation.authorization, Ternary::Unknown);
    assert_eq!(observation.freshness, Ternary::Unknown);
    assert_eq!(observation.evidence, None);
    assert_eq!(observation.usage.total().unwrap(), 0);
}

#[test]
fn evidence_requires_current_revision_and_nonzero_checks() {
    let mut report = CheckReport {
        revision: 7,
        checks: 1,
        result: CheckResult::Passed,
    };
    assert_eq!(report.verdict(7), Ternary::True);
    assert_eq!(report.verdict(8), Ternary::Unknown, "ORCH_STALE_EVIDENCE");
    report.checks = 0;
    assert_eq!(report.verdict(7), Ternary::Unknown, "ORCH_ZERO_CHECKS");
    report.checks = 1;
    report.result = CheckResult::Refuted;
    assert_eq!(report.verdict(7), Ternary::False);
    report.result = CheckResult::Inconclusive;
    assert_eq!(report.verdict(7), Ternary::Unknown);
}

#[test]
fn token_usage_counts_cached_tokens_and_detects_overflow() {
    assert_eq!(
        TokenUsage {
            fresh: 3,
            cached: 5,
            output: 7
        }
        .total()
        .unwrap(),
        15
    );
    assert!(
        TokenUsage {
            fresh: u64::MAX,
            cached: 1,
            output: 0
        }
        .total()
        .is_err()
    );
    assert!(
        TokenUsage {
            fresh: 0,
            cached: u64::MAX,
            output: 1
        }
        .total()
        .is_err()
    );
}
