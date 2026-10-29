use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn run(command: &str, input: &Value) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_trit-orch"))
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("binary should start");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: Value = serde_json::from_str(stdout.trim()).expect("stdout must be json");
    (output.status.code().unwrap_or(-1), value)
}

fn task(id: &str) -> Value {
    json!({
        "task": {"id": id, "priority": 0, "duration": 1, "deps": []},
        "requirements": {"cpu": 1, "memory": 1},
        "max_tokens": 100,
        "epoch": 0
    })
}

fn observation() -> Value {
    json!({
        "phase": "Pending",
        "epoch": 0,
        "authorization": "True",
        "freshness": "True",
        "evidence": null,
        "usage": {"fresh": 0, "cached": 0, "output": 0}
    })
}

fn snapshot(ids: &[&str]) -> Value {
    let mut observations = serde_json::Map::new();
    for id in ids {
        observations.insert((*id).into(), observation());
    }
    json!({"revision": 1, "observations": observations})
}

fn limits() -> Value {
    json!({"workers": 2, "capacity": {"cpu": 2, "memory": 2}, "tokens": 1000})
}

#[test]
fn plan_accepts_valid_graph_and_rejects_cycles() {
    let (code, value) = run("plan", &json!({"revision": 1, "tasks": [task("a")]}));
    assert_eq!(code, 0);
    assert_eq!(value["ok"], true);
    assert_eq!(value["tasks"], 1);

    let cyclic = json!({
        "revision": 1,
        "tasks": [{
            "task": {"id": "a", "priority": 0, "duration": 1, "deps": [{"task": "a", "required": "True"}]},
            "requirements": {"cpu": 1, "memory": 1},
            "max_tokens": 100,
            "epoch": 0
        }]
    });
    let (code, value) = run("plan", &cyclic);
    assert_eq!(code, 1, "ORCH_CLI_EXIT_ON_ERROR");
    assert_eq!(value["ok"], false);
    assert_eq!(value["kind"], "cycle", "ORCH_CLI_ERROR_KIND");
}

#[test]
fn select_reports_batch_and_brief() {
    let input = json!({
        "revision": 1,
        "tasks": [task("a"), task("b")],
        "snapshot": snapshot(&["a", "b"]),
        "limits": limits()
    });
    let (code, value) = run("select", &input);
    assert_eq!(code, 0);
    assert_eq!(value["ok"], true);
    assert_eq!(value["selection"]["selected"], json!(["b", "a"]));
    assert!(value["brief"].as_str().unwrap().contains("selected=2"));
}

#[test]
fn select_applies_a_patch_before_scheduling() {
    let input = json!({
        "revision": 1,
        "tasks": [task("a")],
        "snapshot": snapshot(&["a"]),
        "limits": limits(),
        "patch": {"base_revision": 1, "add": [task("b")], "remove": [], "replace": []},
        "base_snapshot": snapshot(&["a"])
    });
    let (code, value) = run("select", &input);
    assert_eq!(code, 1, "patch 后 revision 变了，旧快照必须被拒");
    assert_eq!(
        value["kind"], "revision_mismatch",
        "ORCH_CLI_STALE_SNAPSHOT"
    );

    let mut fresh = snapshot(&["a", "b"]);
    fresh["revision"] = json!(2);
    let input = json!({
        "revision": 1,
        "tasks": [task("a")],
        "snapshot": fresh,
        "limits": limits(),
        "patch": {"base_revision": 1, "add": [task("b")], "remove": [], "replace": []},
        "base_snapshot": snapshot(&["a"])
    });
    let (code, value) = run("select", &input);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["selection"]["revision"], 2);
    assert_eq!(value["selection"]["selected"], json!(["b", "a"]));
}

#[test]
fn dispatch_requires_prepared_intents() {
    let selection = json!({
        "revision": 1,
        "selected": ["a"],
        "decisions": [],
        "observed_tokens": {"fresh": 0, "cached": 0, "output": 0},
        "reserved_tokens": 100,
        "reserved_resources": {"cpu": 1, "memory": 1},
        "occupied_workers": 1
    });
    let empty = json!({"records": {}});
    let (code, value) = run(
        "dispatch",
        &json!({
            "selection": selection,
            "tasks": [task("a")],
            "ledger": empty
        }),
    );
    assert_eq!(code, 1);
    assert_eq!(value["kind"], "not_prepared", "ORCH_CLI_DISPATCH_GATE");

    let prepared = json!({"records": {"a": {
        "attempt": {"operation": "a", "task": "a", "epoch": 0, "attempt": 1, "fence": 1},
        "state": "Prepared"
    }}});
    let (code, value) = run(
        "dispatch",
        &json!({
            "selection": selection,
            "tasks": [task("a")],
            "ledger": prepared
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["ready"].as_array().unwrap().len(), 1);
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
            "rounds": []
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["summary"]["confirmed"], 1);
}

#[test]
fn recover_classifies_durable_states() {
    let ledger = json!({"records": {
        "ready-op": {"attempt": {"operation": "ready-op", "task": "t", "epoch": 0, "attempt": 1, "fence": 1}, "state": "Prepared"},
        "doubt-op": {"attempt": {"operation": "doubt-op", "task": "t", "epoch": 0, "attempt": 1, "fence": 1}, "state": "Sending"},
        "done-op": {"attempt": {"operation": "done-op", "task": "t", "epoch": 0, "attempt": 1, "fence": 1}, "state": "Confirmed"}
    }});
    let (code, value) = run("recover", &ledger);
    assert_eq!(code, 0);
    assert_eq!(value["recovery"]["ready"], json!(["ready-op"]));
    assert_eq!(value["recovery"]["in_doubt"], json!(["doubt-op"]));
    assert_eq!(value["recovery"]["done"], json!(["done-op"]));
}

#[test]
fn bad_input_and_unknown_commands_fail_closed() {
    let (code, value) = run("plan", &json!({"revision": 1}));
    assert_eq!(code, 1);
    assert_eq!(value["kind"], "input_parse", "ORCH_CLI_INPUT_PARSE");

    let (code, value) = run("frobnicate", &json!({}));
    assert_eq!(code, 1);
    assert_eq!(value["kind"], "input_parse");
    assert!(value["error"].as_str().unwrap().contains("未知子命令"));
}

#[test]
fn shadow_replay_reports_unsafe_proposals_without_dispatching() {
    let spec = json!({
        "task": {"id": "a", "priority": 0, "duration": 1, "deps": []},
        "requirements": {"cpu": 1, "memory": 1},
        "max_tokens": 100,
        "epoch": 0
    });
    let mut observation = observation();
    observation["phase"] = json!("InFlight");
    let step = json!({
        "revision": 1,
        "tasks": [spec],
        "snapshot": {"revision": 1, "observations": {"a": observation}},
        "limits": limits(),
        "ledger": {"records": {"a": {
            "attempt": {"operation": "a", "task": "a", "epoch": 0, "attempt": 1, "fence": 1},
            "state": "Sending"
        }}}
    });
    let (code, value) = run("shadow", &json!({"policy": "eager", "steps": [step]}));
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["safe"], false, "ORCH_CLI_SHADOW_SAFE");
    assert_eq!(value["report"]["unsafe_extra"], json!(["0:a"]));
    assert_eq!(value["report"]["steps"], 1);
}

fn run_with_args(command: &str, extra: &[&str], input: &Value) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_trit-orch"))
        .arg(command)
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("binary should start");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let value: Value =
        serde_json::from_str(String::from_utf8(output.stdout).unwrap().trim()).unwrap();
    (output.status.code().unwrap_or(-1), value)
}

#[test]
fn select_can_record_traces_and_shadow_replays_them() {
    let mut trace = std::env::temp_dir();
    trace.push(format!("trit-orch-cli-trace-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&trace);
    let trace_arg = trace.to_string_lossy().to_string();

    let request = json!({
        "revision": 1,
        "tasks": [task("a")],
        "snapshot": snapshot(&["a"]),
        "limits": limits()
    });
    for _ in 0..2 {
        let (code, value) = run_with_args("select", &["--capture", &trace_arg], &request);
        assert_eq!(code, 0, "{value}");
    }
    let text = std::fs::read_to_string(&trace).unwrap_or_default();
    let lines = text.lines().count();
    assert_eq!(lines, 2, "ORCH_CLI_CAPTURE: 每次 select 追加一行");

    let (code, value) = run_with_args("shadow-jsonl", &["critical_path", &trace_arg], &json!({}));
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["report"]["steps"], 2);
    assert_eq!(value["safe"], true);
    let _ = std::fs::remove_file(&trace);
}
