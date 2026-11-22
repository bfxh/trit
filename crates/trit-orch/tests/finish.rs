use serde_json::json;

mod cli_support;
use cli_support::*;

#[test]
fn shared_fixtures_are_stable() {
    assert_eq!(limits()["workers"], 2);
    assert_eq!(context_facts(json!({}))["measured_tokens"], 60000);
}

#[test]
fn finish_refuses_incomplete_plans_and_reports_complete_ones() {
    let (code, value) = run(
        "finish",
        &json!({
            "tasks": [task("a")],
            "snapshot": snapshot(&["a"]),
            "rounds": []
        }),
    );
    assert_eq!(code, 1);
    assert_eq!(value["kind"], "incomplete", "ORCH_CLI_FINISH_GATE");

    let mut settled = snapshot(&["a"]);
    settled["observations"]["a"] = json!({
        "phase": "Settled",
        "epoch": 0,
        "authorization": "True",
        "freshness": "True",
        "evidence": {"epoch": 0, "checks": 2, "result": "Passed"},
        "usage": {"fresh": 10, "cached": 0, "output": 5}
    });
    let (code, value) = run(
        "finish",
        &json!({
            "tasks": [task("a")],
            "snapshot": settled,
            "rounds": [],
            "receipts": {
                "a": {
                    "epoch": 0,
                    "exit": {"Completed": {"code": 0}},
                    "assertions_passed": 2,
                    "assertions_failed": 0
                }
            }
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["summary"]["confirmed"], 1);

    let mut forged = snapshot(&["a"]);
    forged["observations"]["a"] = json!({
        "phase": "Settled",
        "epoch": 0,
        "authorization": "True",
        "freshness": "True",
        "evidence": {"epoch": 0, "checks": 2, "result": "Passed"},
        "usage": {"fresh": 10, "cached": 0, "output": 5}
    });
    let (code, value) = run(
        "finish",
        &json!({
            "tasks": [task("a")],
            "snapshot": forged,
            "rounds": []
        }),
    );
    assert_eq!(code, 1);
    assert_eq!(
        value["kind"], "unattested_evidence",
        "ORCH_CLI_FINISH_NO_RECEIPT"
    );
}

#[test]
fn legacy_allow_unattested_is_explicit_only() {
    let mut forged = snapshot(&["a"]);
    forged["observations"]["a"] = json!({
        "phase": "Settled",
        "epoch": 0,
        "authorization": "True",
        "freshness": "True",
        "evidence": {"epoch": 0, "checks": 2, "result": "Passed"},
        "usage": {"fresh": 10, "cached": 0, "output": 5}
    });
    let (code, value) = run_with_args(
        "finish",
        &["--allow-unattested"],
        &json!({
            "tasks": [task("a")],
            "snapshot": forged,
            "rounds": []
        }),
    );
    assert_eq!(code, 0, "legacy flag must be explicit: {value}");
    assert_eq!(value["summary"]["confirmed"], 1);
}
