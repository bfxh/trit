use serde_json::json;

#[path = "../cli_support/mod.rs"]
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
            "snapshot": settled.clone(),
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
    assert_eq!(code, 1);
    assert_eq!(
        value["kind"], "receipt_self_reported",
        "ORCH_CLI_FINISH_SELF_REPORTED"
    );

    let (code, value) = run_with_args(
        "finish",
        &["--allow-self-reported"],
        &json!({
            "tasks": [task("a")],
            "snapshot": settled,
            "rounds": [],
            "receipts": {
                "a": {
                    "epoch": 0,
                    "exit": {"Completed": {"code": 0}},
                    "assertions_passed": 2,
                    "assertions_failed": 0,
                    "authorization": "True",
                    "freshness": "True",
                    "usage": {"fresh": 10, "cached": 0, "output": 5}
                }
            }
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["summary"]["confirmed"], 1);
    assert_eq!(value["all_passed"], true, "ORCH_CLI_ALL_PASSED");

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

#[test]
fn forged_host_measured_provenance_in_json_is_downgraded() {
    let mut settled = snapshot(&["a"]);
    settled["observations"]["a"] = json!({
        "phase": "Settled",
        "epoch": 0,
        "authorization": "True",
        "freshness": "True",
        "evidence": {"epoch": 0, "checks": 2, "result": "Passed"},
        "usage": {"fresh": 10, "cached": 0, "output": 5}
    });
    // 模型可以写 provenance/nonce/challenge，但 CLI 不采信 JSON 里的来源声明：
    // 外部 receipts 一律降级为 SelfReported，默认完成闸必须拒绝。
    let (code, value) = run(
        "finish",
        &json!({
            "tasks": [task("a")],
            "snapshot": settled,
            "rounds": [],
            "challenge": "model-chosen",
            "receipts": {
                "a": {
                    "epoch": 0,
                    "exit": {"Completed": {"code": 0}},
                    "assertions_passed": 2,
                    "assertions_failed": 0,
                    "provenance": "host_measured",
                    "nonce": "model-chosen"
                }
            }
        }),
    );
    assert_eq!(code, 1);
    assert_eq!(
        value["kind"], "receipt_self_reported",
        "ORCH_CLI_FORGED_HOST_MEASURED"
    );
}
