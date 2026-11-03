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
    trace.push(format!(
        "trit-orch-cli-trace-{}-{:?}.jsonl",
        std::process::id(),
        std::thread::current().id()
    ));
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

fn context_facts(extra: Value) -> Value {
    let mut facts = json!({
        "measured_tokens": 60000,
        "context_window": 100000,
        "threshold_bp": 5100,
        "prunable_tokens": 0,
        "overflow": "not_reported",
        "durable_flushed": "Unknown",
        "recall_reachable": "Unknown",
        "constraints_pinned": "Unknown",
        "progress_since_compaction": "Unknown"
    });
    for (key, value) in extra.as_object().unwrap() {
        facts[key] = value.clone();
    }
    facts
}

#[test]
fn compact_gate_reports_unapproved_without_touching_history() {
    let (code, value) = run("compact-gate", &context_facts(json!({})));
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["ok"], true, "ORCH_CTX_CLI");
    assert_eq!(value["decision"]["admission"], "Unknown", "ORCH_CTX_CLI");
    assert_eq!(value["decision"]["tier"], "keep", "ORCH_CTX_CLI");
    assert_eq!(value["decision"]["need"], "True", "ORCH_CTX_CLI: 压力仍在");
    assert!(
        value["decision"]["blocks"]
            .as_array()
            .unwrap()
            .contains(&json!("facts_not_flushed")),
        "ORCH_CTX_CLI"
    );
}

#[test]
fn compact_gate_approves_prune_over_the_lossy_tier() {
    let (code, value) = run(
        "compact-gate",
        &context_facts(json!({"prunable_tokens": 9000, "recall_reachable": "True"})),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["decision"]["tier"], "prune", "ORCH_CTX_CLI_PRUNE");
    assert_eq!(value["decision"]["admission"], "True", "ORCH_CTX_CLI_PRUNE");
}

#[test]
fn compact_gate_bad_config_uses_stable_error_kind() {
    let (code, value) = run(
        "compact-gate",
        &context_facts(json!({"threshold_bp": 4999})),
    );
    assert_eq!(code, 1, "ORCH_CTX_CLI_CONFIG");
    assert_eq!(value["ok"], false, "ORCH_CTX_CLI_CONFIG");
    assert_eq!(value["kind"], "context_config", "ORCH_CTX_CLI_CONFIG");
}

#[test]
fn claim_cli_acquire_renew_release_and_gate() {
    let (code, value) = run(
        "claim",
        &json!({
            "owner": "alice",
            "paths": ["src/a.rs"],
            "intent": "edit",
            "now": 100,
            "ttl": 10
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["outcome"]["Acquired"]["owner"], "alice");
    let claim_id = value["outcome"]["Acquired"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let fencing = value["outcome"]["Acquired"]["fencing"].as_u64().unwrap();
    let store = value["store"].clone();

    let (code, value) = run(
        "claim-gate",
        &json!({"store": store, "owner": "alice", "paths": ["src/a.rs"], "now": 101}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "True");

    let (code, value) = run(
        "renew",
        &json!({
            "store": store,
            "id": claim_id,
            "owner": "alice",
            "fencing": fencing,
            "now": 101,
            "ttl": 20
        }),
    );
    assert_eq!(code, 0, "{value}");
    let renewed = value["claim"]["fencing"].as_u64().unwrap();
    assert!(renewed > fencing);
    let store = value["store"].clone();

    let (code, value) = run(
        "release",
        &json!({"store": store, "id": claim_id, "owner": "alice", "fencing": renewed}),
    );
    assert_eq!(code, 0, "{value}");
    let store = value["store"].clone();

    let (code, value) = run(
        "claim-gate",
        &json!({"store": store, "owner": "alice", "paths": ["src/a.rs"], "now": 102}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "Unknown", "ORCH_CLI_CLAIM_RELEASE");
}

#[test]
fn claim_cli_conflict_is_contested_and_gate_blocks() {
    let (code, value) = run(
        "claim",
        &json!({
            "owner": "alice",
            "paths": ["src/a.rs"],
            "intent": "edit",
            "now": 100,
            "ttl": 10
        }),
    );
    assert_eq!(code, 0, "{value}");
    let store = value["store"].clone();

    let (code, value) = run(
        "claim",
        &json!({
            "store": store,
            "owner": "bob",
            "paths": ["src"],
            "intent": "edit",
            "now": 101,
            "ttl": 10
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        value["outcome"]["Contested"]["contenders"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let store = value["store"].clone();

    let (code, value) = run(
        "claim-gate",
        &json!({"store": store, "owner": "bob", "paths": ["src/a.rs"], "now": 101}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "False", "ORCH_CLI_CLAIM_CONFLICT");
}

#[test]
fn receipt_cli_walks_four_states_and_rejects_skip() {
    let (code, value) = run(
        "receipt",
        &json!({"action": "record", "id": "r1", "key": "k1"}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["receipt"]["state"], "Sent");
    let ledger = value["ledger"].clone();

    let (code, value) = run(
        "receipt",
        &json!({"ledger": ledger, "action": "advance", "key": "k1", "state": "Delivered"}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["receipt"]["state"], "Delivered");
    let ledger = value["ledger"].clone();

    let (code, value) = run(
        "receipt",
        &json!({"ledger": ledger, "action": "advance", "key": "k1", "state": "Acted"}),
    );
    assert_eq!(code, 1, "ORCH_CLI_RECEIPT_SKIP: 不能跳过 Read");
    assert_eq!(value["kind"], "receipt_skip");
}

#[test]
fn handoff_gate_cli_three_states() {
    let packet = json!({
        "id": "h1",
        "from": "alice",
        "to": "bob",
        "goal": "finish",
        "base_commit": "abc",
        "state_hash": "sha256:abc",
        "decisions": [],
        "open_questions": [],
        "claims": [],
        "evidence": [],
        "next_action": "run gates",
        "idempotency_key": "handoff:h1",
        "expires_at": 200
    });
    let (code, value) = run(
        "handoff-gate",
        &json!({"packet": packet, "receiver_state_hash": "sha256:abc", "now": 100}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "True");

    let (code, value) = run(
        "handoff-gate",
        &json!({"packet": packet, "receiver_state_hash": "sha256:other", "now": 100}),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "Unknown", "ORCH_CLI_HANDOFF_STATE");
}

#[test]
fn admit_with_claim_cli_is_k3_and() {
    let (code, value) = run(
        "admit-with-claim",
        &json!({
            "dependencies": "True",
            "authorization": "True",
            "resources": "True",
            "freshness": "True",
            "claim": "True"
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "True");

    let (code, value) = run(
        "admit-with-claim",
        &json!({
            "dependencies": "True",
            "authorization": "Unknown",
            "resources": "True",
            "freshness": "True",
            "claim": "True"
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "Unknown");

    let (code, value) = run(
        "admit-with-claim",
        &json!({
            "dependencies": "True",
            "authorization": "True",
            "resources": "True",
            "freshness": "True",
            "claim": "False"
        }),
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["gate"], "False", "ORCH_CLI_ADMIT_CLAIM");
}
