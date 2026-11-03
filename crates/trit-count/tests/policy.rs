use std::collections::HashSet;

use trit_core::value::Ternary;
use trit_count::count_models_under;
use trit_count::oracle::count_exhaustive_under;
use trit_count::policy::{Assertion, Policy, PolicyError};

const EXAMPLE: &str = include_str!("../../../examples/policy.json");
const BAD: &str = include_str!("../../trit-cli/tests/fixtures/bad_policy.json");

fn targets(policy: &Policy, assertion: &Assertion) -> [u128; 3] {
    let mut out = [0u128; 3];
    for (slot, target) in [Ternary::False, Ternary::True, Ternary::Unknown]
        .into_iter()
        .enumerate()
    {
        out[slot] = count_models_under(
            &assertion.expr,
            &policy.assumptions,
            &policy.vars,
            &policy.binary,
            policy.algebra,
            target,
        )
        .count;
        let exhaustive = count_exhaustive_under(
            &assertion.expr,
            &policy.assumptions,
            &policy.vars,
            &policy.binary,
            policy.algebra,
            target,
        );
        assert_eq!(
            out[slot], exhaustive,
            "dpll vs exhaustive mismatch for assertion {:?}",
            assertion.name
        );
    }
    out
}

#[test]
fn example_policy_expands_and_counts() {
    let policy = Policy::from_json(EXAMPLE).expect("example policy");
    assert_eq!(policy.vars, ["contended", "hopeless", "infeasible"]);
    let mut binary: Vec<String> = policy.binary.iter().cloned().collect();
    binary.sort();
    assert_eq!(binary, ["contended", "infeasible"]);
    assert_eq!(policy.assertions.len(), 2);

    let assumption_true = policy.count_assumptions();
    assert_eq!(assumption_true, 2 * 2 * 3);
    for assertion in &policy.assertions {
        let counts = targets(&policy, assertion);
        assert_eq!(counts.iter().sum::<u128>(), assumption_true);
        assert_eq!(counts[1], assumption_true, "assertion {:?}", assertion.name);
        assert_eq!(counts[0], 0);
        assert_eq!(counts[2], 0);
    }
}

#[test]
fn bad_policy_exposes_the_violating_assignment() {
    let policy = Policy::from_json(BAD).expect("bad policy fixture");
    let assumption_true = policy.count_assumptions();
    let assertion = &policy.assertions[0];
    let counts = targets(&policy, assertion);
    assert_eq!(counts.iter().sum::<u128>(), assumption_true);
    assert_eq!(counts[0], 1, "exactly one assignment must falsify it");
    assert_eq!(counts[1], 9);
    assert_eq!(counts[2], 2);
}

#[test]
fn assumptions_restrict_policy_counts() {
    let source = r#"{
        "algebra": "kleene",
        "binary": ["b"],
        "assumptions": ["a"],
        "assertions": [{ "name": "or", "expr": "a | b" }]
    }"#;
    let policy = Policy::from_json(source).expect("inline policy");
    assert_eq!(policy.vars, ["a", "b"]);
    let assumption_true = policy.count_assumptions();
    assert_eq!(assumption_true, 2);
    let counts = targets(&policy, &policy.assertions[0]);
    assert_eq!(counts.iter().sum::<u128>(), 2);
    assert_eq!(counts[1], 2);
    assert_eq!(counts[0], 0);
    assert_eq!(counts[2], 0);
}

#[test]
fn unused_binary_variable_extends_the_domain() {
    let source = r#"{
        "binary": ["z"],
        "assertions": [{ "name": "single", "expr": "a" }]
    }"#;
    let policy = Policy::from_json(source).expect("inline policy");
    assert_eq!(policy.vars, ["a", "z"]);
    assert_eq!(policy.count_assumptions(), 6);
    let counts = targets(&policy, &policy.assertions[0]);
    assert_eq!(counts.iter().sum::<u128>(), 6);
    assert_eq!(counts, [2, 2, 2]);
}

#[test]
fn recursive_definitions_are_rejected() {
    let source = r#"{
        "definitions": { "p": "q", "q": "p" },
        "assertions": [{ "name": "x", "expr": "p" }]
    }"#;
    let error = Policy::from_json(source).expect_err("cycle must be rejected");
    assert!(
        matches!(error, PolicyError::RecursiveDefinition(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn binary_named_after_a_definition_is_rejected() {
    let source = r#"{
        "definitions": { "reject": "a" },
        "binary": ["reject"],
        "assertions": [{ "name": "x", "expr": "reject" }]
    }"#;
    let error = Policy::from_json(source).expect_err("shadowing must be rejected");
    assert_eq!(error, PolicyError::BinaryIsDefinition("reject".to_string()));
}

#[test]
fn empty_assertion_set_is_rejected() {
    let error = Policy::from_json(r#"{ "assertions": [] }"#)
        .expect_err("policy without assertions must fail");
    assert_eq!(error, PolicyError::NoAssertions);
}

#[test]
fn policy_unknown_top_level_field_is_rejected() {
    let error = Policy::from_json(
        r#"{ "assertions": [{ "name": "x", "expr": "a" }], "algebraa": "kleene" }"#,
    )
    .expect_err("typo in top-level field must fail");
    assert!(
        matches!(error, PolicyError::Json(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn default_algebra_is_kleene() {
    let policy = Policy::from_json(r#"{ "assertions": [{ "name": "x", "expr": "a & b" }] }"#)
        .expect("inline");
    assert_eq!(policy.algebra, trit_core::algebra::Algebra::Kleene);
    assert_eq!(policy.vars, ["a", "b"]);
}

#[test]
fn binary_set_matches_declared_names() {
    let policy = Policy::from_json(EXAMPLE).expect("example policy");
    let expected: HashSet<String> = ["contended", "infeasible"]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(policy.binary, expected);
}
