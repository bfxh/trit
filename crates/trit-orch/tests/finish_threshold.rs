use serde_json::json;

#[allow(dead_code)]
#[path = "cli_support/mod.rs"]
mod cli_support;
use cli_support::*;
#[test]
fn task_min_assertions_is_authoritative_in_cli_finish() {
    let mut task_a = task("a");
    task_a["min_assertions"] = json!(3);
    let mut settled = snapshot(&["a"]);
    settled["observations"]["a"] = json!({
        "phase": "Settled",
        "epoch": 0,
        "authorization": "True",
        "freshness": "True",
        "evidence": {"epoch": 0, "checks": 2, "result": "Passed"},
        "usage": {"fresh": 10, "cached": 0, "output": 5}
    });
    let receipt = json!({
        "epoch": 0,
        "exit": {"Completed": {"code": 0}},
        "assertions_passed": 2,
        "assertions_failed": 0,
        "authorization": "True",
        "freshness": "True",
        "usage": {"fresh": 10, "cached": 0, "output": 5}
    });
    // 没给全局 min_assertions：旧行为会用默认 1 放过 2 条断言，
    // 新行为必须按 TaskSpec 的 3 判，回执与快照不一致。
    let (code, value) = run_with_args(
        "finish",
        &["--allow-self-reported"],
        &json!({
            "tasks": [task_a],
            "snapshot": settled,
            "rounds": [],
            "receipts": {"a": receipt}
        }),
    );
    assert_eq!(code, 1);
    assert_eq!(value["kind"], "receipt_mismatch", "ORCH_CLI_TASK_THRESHOLD");
}
