//! gate 单测。

use super::*;
use trit_learn::json::Json;

fn td(s: &str) -> Trajectory {
    Trajectory::from_json(&Json::parse(s).unwrap()).unwrap()
}

const OK: &str = r#"{"task_id":"t1","model":"m","steps":[{"role":"assistant","thought":"I will run the test suite now to check the build","action":{"tool":"bash","args":{"cmd":"pytest -q"}},"observation":"3 passed"},{"role":"assistant","thought":"All tests pass so I can finish this task","action":{"tool":"finish","args":{}},"observation":"done"}],"outcome":{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"final":"fixed"}}"#;

#[test]
fn accepts_a_clean_trajectory() {
    let mut seen = BTreeSet::new();
    let v = evaluate(&td(OK), &Config::default(), &mut seen);
    assert_eq!(v.decision, Decision::Accept, "{:?}", v.reasons);
}

#[test]
fn rejects_unverified_and_missing_evidence() {
    let mut seen = BTreeSet::new();
    let t = td(&OK.replace(r#""passed":true"#, r#""passed":false"#));
    let v = evaluate(&t, &Config::default(), &mut seen);
    assert_eq!(v.decision, Decision::Reject);
    assert!(v.reasons.contains(&"verified"));
}

#[test]
fn rejects_loops_and_duplicates() {
    let looped = r#"{"task_id":"t2","model":"m","steps":[{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"a"},{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"b"},{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"c"}],"outcome":{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]}}"#;
    let mut seen = BTreeSet::new();
    let v = evaluate(&td(looped), &Config::default(), &mut seen);
    assert_eq!(v.decision, Decision::Reject);
    assert!(v.reasons.contains(&"loop"));

    let mut seen = BTreeSet::new();
    assert_eq!(
        evaluate(&td(OK), &Config::default(), &mut seen).decision,
        Decision::Accept
    );
    let v = evaluate(&td(OK), &Config::default(), &mut seen);
    assert!(v.reasons.contains(&"dedup"));
}

#[test]
fn defers_on_process_or_language_gaps() {
    let mut seen = BTreeSet::new();
    let no_obs = td(&OK.replace(r#""observation":"3 passed""#, r#""observation":"""#));
    let v = evaluate(&no_obs, &Config::default(), &mut seen);
    assert_eq!(v.decision, Decision::Defer);
    assert!(v.reasons.contains(&"observation"));

    let mut seen = BTreeSet::new();
    let no_thought = td(&OK
        .replace("I will run the test suite now to check the build", "")
        .replace("All tests pass so I can finish this task", ""));
    let v = evaluate(&no_thought, &Config::default(), &mut seen);
    assert_eq!(v.decision, Decision::Defer);
    assert!(v.reasons.contains(&"thought"));
}

#[test]
fn defers_on_bad_language() {
    let bad = "重复句子。".repeat(80);
    let json = format!(
        r#"{{"task_id":"t3","model":"m","steps":[{{"role":"assistant","thought":"{bad}","action":{{"tool":"bash","args":{{"cmd":"ls"}}}},"observation":"{bad}"}},{{"role":"assistant","thought":"{bad}","action":{{"tool":"finish","args":{{}}}},"observation":"{bad}"}}],"outcome":{{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],"final":"{bad}"}}}}"#
    );
    let mut seen = BTreeSet::new();
    let v = evaluate(&td(&json), &Config::default(), &mut seen);
    assert_eq!(v.decision, Decision::Defer, "{:?}", v.reasons);
    assert!(v.reasons.contains(&"language"));
}
#[test]
fn language_deny_and_disable_are_configurable() {
    let mut seen = BTreeSet::new();
    let deny = Config {
        lang_deny: vec!["en".into()],
        ..Config::default()
    };
    let v = evaluate(&td(OK), &deny, &mut seen);
    assert_eq!(v.decision, Decision::Defer);
    assert!(v.reasons.contains(&"lang_denied"));

    let mut seen = BTreeSet::new();
    let off = Config {
        language_gate: false,
        ..Config::default()
    };
    assert_eq!(
        evaluate(&td(OK), &off, &mut seen).decision,
        Decision::Accept
    );

    let mut seen = BTreeSet::new();
    let allow = Config {
        lang_allow: Some(vec!["zh".into()]),
        ..Config::default()
    };
    let v = evaluate(&td(OK), &allow, &mut seen);
    assert_eq!(v.decision, Decision::Defer);
    assert!(v.reasons.contains(&"lang_not_allowed"));
}

#[test]
fn dedup_and_loop_can_be_disabled() {
    let cfg = Config {
        dedup: false,
        ..Config::default()
    };
    let mut seen = BTreeSet::new();
    assert_eq!(
        evaluate(&td(OK), &cfg, &mut seen).decision,
        Decision::Accept
    );
    assert_eq!(
        evaluate(&td(OK), &cfg, &mut seen).decision,
        Decision::Accept
    );

    let looped = r#"{"task_id":"t2","model":"m","steps":[{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"a"},{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"b"},{"role":"assistant","thought":"run","action":{"tool":"bash","args":{"cmd":"ls"}},"observation":"c"}],"outcome":{"verifier":"pytest","passed":true,"evidence":["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]}}"#;
    let cfg = Config {
        loop_gate: false,
        min_thought_rate: 0.0,
        ..Config::default()
    };
    let mut seen = BTreeSet::new();
    let v = evaluate(&td(looped), &cfg, &mut seen);
    assert!(!v.reasons.contains(&"loop"), "{:?}", v.reasons);
}

#[path = "gate_lang_tests.rs"]
mod lang;
