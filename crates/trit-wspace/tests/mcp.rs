//! MCP 工具面：协议、六个工具、回滚与错误。

use serde_json::{Value, json};
use trit_wspace::mcp::{Mcp, PROTOCOL_VERSION, tools};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit, from_wsp, to_wsp};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn xor_artifact() -> String {
    to_wsp(&xor_net())
}

fn xor_oracle() -> Value {
    json!({"domain": "boolean", "vars": 2, "table": "0110"})
}

fn call(server: &mut Mcp, name: &str, args: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": name, "arguments": args},
    });
    let response = server.dispatch(&request).expect("response");
    assert_eq!(response["jsonrpc"], "2.0");
    response["result"].clone()
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .expect("text content")
        .to_string()
}

fn payload(result: &Value) -> Value {
    serde_json::from_str(&text(result)).expect("json payload")
}

#[test]
fn initialize_reports_protocol_and_server_name() {
    let mut server = Mcp::new();
    let response = server
        .dispatch(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}))
        .expect("response");
    assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(response["result"]["serverInfo"]["name"], "trit-wspace");
    assert!(response["result"]["capabilities"]["tools"].is_object());
}

#[test]
fn lists_exactly_fifteen_tools() {
    let mut server = Mcp::new();
    let response = server
        .dispatch(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .expect("response");
    let listed = response["result"]["tools"].as_array().expect("tools");
    assert_eq!(listed.len(), 15);
    let names: Vec<&str> = listed
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    for expected in [
        "wsp_read",
        "wsp_verify",
        "wsp_patch",
        "wsp_apply_candidate",
        "wsp_repair",
        "wsp_propose",
        "wsp_explain",
        "wsp_probe",
        "wsp_localize",
        "wsp_audit",
        "wsp_diff",
        "wsp_merge",
        "wsp_shard",
        "wsp_audit_shards",
        "wsp_run_script",
    ] {
        assert!(names.contains(&expected), "缺少工具 {expected}");
    }
    assert_eq!(tools().len(), 15);
}

#[test]
fn read_returns_summary_and_canonical_artifact() {
    let mut server = Mcp::new();
    let result = call(&mut server, "wsp_read", json!({"artifact": xor_artifact()}));
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["summary"]["arch"], json!([2, 2, 1]));
    let text = value["wsp1"].as_str().expect("wsp1");
    assert!(text.starts_with("wsp1\n"));
    assert_eq!(
        from_wsp(text).expect("parse").state_hash(),
        xor_net().state_hash()
    );
}

#[test]
fn verify_returns_witnesses_for_broken_artifact() {
    let mut broken = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_verify",
        json!({"artifact": to_wsp(&broken), "oracle": xor_oracle(), "limit": 4}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["exact"], false);
    assert!(value["loss"].as_u64().expect("loss") >= 1);
    let items = value["mismatches"]["items"].as_array().expect("items");
    assert!(!items.is_empty());
    assert!(items[0]["input"].is_string());
    assert!(items[0]["want"].is_string());
    assert!(items[0]["got"].is_string());
}

#[test]
fn patch_rolls_back_when_postcondition_fails() {
    let artifact = xor_artifact();
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_patch",
        json!({
            "artifact": artifact,
            "patch": {"version": 1, "edits": [
                {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "0"}
            ]},
            "oracle": xor_oracle(),
            "require_after": true,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["report"]["applied"], true);
    assert_eq!(value["report"]["rolled_back"], true);
    let final_text = value["artifact"].as_str().expect("artifact");
    assert_eq!(
        from_wsp(final_text).expect("parse").state_hash(),
        xor_net().state_hash()
    );
}

#[test]
fn patch_applies_without_postcondition() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_patch",
        json!({
            "artifact": xor_artifact(),
            "patch": {"version": 1, "edits": [
                {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "0"}
            ]},
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["report"]["applied"], true);
    assert_eq!(value["report"]["rolled_back"], false);
    assert_eq!(value["report"]["cost"]["touched"].as_u64(), Some(1));
}

#[test]
fn repair_finds_the_one_edit_fix() {
    let mut broken = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_repair",
        json!({"artifact": to_wsp(&broken), "oracle": xor_oracle(), "max_edits": 2}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["found"], true);
    assert_eq!(value["repair"]["after_loss"].as_u64(), Some(0));
    assert_eq!(
        value["repair"]["patch"]["edits"]
            .as_array()
            .expect("edits")
            .len(),
        1
    );
    assert_eq!(value["verify"]["exact"], true);
    let fixed = from_wsp(value["artifact"].as_str().expect("artifact")).expect("parse");
    assert!(xor_oracle_net_check(&fixed));
}

fn xor_oracle_net_check(net: &Net) -> bool {
    use trit_wspace::Oracle;
    Oracle::boolean(2, |bits| bits[0] != bits[1])
        .expect("oracle")
        .exact(net)
        .expect("exact")
}

#[test]
fn propose_returns_replayable_ranked_candidates() {
    let mut broken = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_propose",
        json!({"artifact": to_wsp(&broken), "oracle": xor_oracle(), "limit": 4}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["depth"].as_u64(), Some(1));
    assert!(value["before_loss"].as_u64().expect("before") > 0);
    let candidates = value["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty() && candidates.len() <= 4);
    let top = &candidates[0];
    assert!(top["gain"].as_i64().expect("gain") > 0);
    assert_eq!(top["after_loss"].as_u64(), Some(0));

    let patch = trit_wspace::wire::patch_from_value(&json!({
        "version": 1,
        "edits": [top["edit"].clone()],
    }))
    .expect("returned edit must be a valid patch");
    let mut fixed = broken.clone();
    patch.apply(&mut fixed).expect("apply");
    assert!(xor_oracle_net_check(&fixed));
}

#[test]
fn propose_pairs_returns_two_edit_candidates() {
    let mut broken = xor_net();
    Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 1,
            row: 0,
            col: 0,
            to: Trit::Pos,
        },
    ])
    .apply(&mut broken)
    .expect("corrupt");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_propose",
        json!({
            "artifact": to_wsp(&broken),
            "oracle": xor_oracle(),
            "pairs": true,
            "limit": 4,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["depth"].as_u64(), Some(2));
    let candidates = value["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty() && candidates.len() <= 4);
    let top = &candidates[0];
    assert_eq!(top["after_loss"].as_u64(), Some(0));
    let edits = top["edits"].as_array().expect("edits");
    assert_eq!(edits.len(), 2);

    let patch = trit_wspace::wire::patch_from_value(&json!({
        "version": 1,
        "edits": edits,
    }))
    .expect("pair must be a valid patch");
    let mut fixed = broken.clone();
    patch.apply(&mut fixed).expect("apply");
    assert!(xor_oracle_net_check(&fixed));
}

#[test]
fn propose_depth3_returns_three_edit_candidates() {
    let mut broken = xor_net();
    Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 0,
            row: 0,
            col: 1,
            to: Trit::Zero,
        },
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 0,
            to: Trit::Zero,
        },
    ])
    .apply(&mut broken)
    .expect("corrupt");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_propose",
        json!({
            "artifact": to_wsp(&broken),
            "oracle": xor_oracle(),
            "depth": 3,
            "limit": 4,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["depth"].as_u64(), Some(3));
    let candidates = value["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty() && candidates.len() <= 4);
    let top = &candidates[0];
    assert_eq!(top["after_loss"].as_u64(), Some(0));
    let edits = top["edits"].as_array().expect("edits");
    assert_eq!(edits.len(), 3);

    let patch = trit_wspace::wire::patch_from_value(&json!({
        "version": 1,
        "edits": edits,
    }))
    .expect("triple must be a valid patch");
    let mut fixed = broken.clone();
    patch.apply(&mut fixed).expect("apply");
    assert!(xor_oracle_net_check(&fixed));
}

#[test]
fn apply_candidate_uses_proposal_and_appends_log() {
    let mut broken = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    let candidate = json!({
        "edit": {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "-"}
    });
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_apply_candidate",
        json!({
            "artifact": to_wsp(&broken),
            "candidate": candidate,
            "oracle": xor_oracle(),
            "require_after": true,
            "log": "",
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["applied"], true);
    assert_eq!(value["rolled_back"], false);
    assert_eq!(value["verify"]["exact"], true);
    let log_text = value["log"].as_str().expect("log");
    let log = trit_wspace::AuditLog::from_jsonl(log_text).expect("parse log");
    log.verify_chain().expect("chain");
    assert_eq!(log.len(), 1);
    let (replay, _) = log.replay_net(&broken).expect("replay");
    assert_eq!(replay.behavior_checked, 1);
}

#[test]
fn apply_candidate_rolls_back_and_logs_it() {
    let candidate = json!({
        "edit": {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "0"}
    });
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_apply_candidate",
        json!({
            "artifact": xor_artifact(),
            "candidate": candidate,
            "oracle": xor_oracle(),
            "require_after": true,
            "log": "",
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["applied"], true);
    assert_eq!(value["rolled_back"], true);
    assert_eq!(value["verify"]["exact"], true, "回滚后仍是原工件");
    let log =
        trit_wspace::AuditLog::from_jsonl(value["log"].as_str().expect("log")).expect("parse log");
    log.verify_chain().expect("chain");
    let entry = &log.entries()[0];
    assert!(entry.rolled_back);
    assert_eq!(entry.before, entry.after);
}

#[test]
fn apply_candidate_rejects_empty_candidate() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_apply_candidate",
        json!({"artifact": xor_artifact(), "candidate": {}}),
    );
    assert_eq!(result["isError"], true);
}

#[test]
fn explain_reports_sample_and_fixing_edit() {
    let mut broken = xor_net();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_explain",
        json!({"artifact": to_wsp(&broken), "oracle": xor_oracle(), "sample": 0, "top": 4}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["index"].as_u64(), Some(0));
    assert_eq!(value["want"].as_str(), Some("-"));
    assert_eq!(value["got"].as_str(), Some("0"));
    assert_eq!(value["fully_evaluated"].as_u64(), Some(4));
    let top = &value["candidates"][0];
    assert_eq!(top["fixes"], true);
    assert_eq!(top["after_loss"].as_u64(), Some(0));

    let patch = trit_wspace::wire::patch_from_candidate(&json!({"edit": top["edit"].clone()}))
        .expect("edit");
    let mut fixed = broken.clone();
    patch.apply(&mut fixed).expect("apply");
    assert!(xor_oracle_net_check(&fixed));
}

#[test]
fn probe_returns_row_ablation() {
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_probe",
        json!({"artifact": xor_artifact(), "oracle": xor_oracle(), "top": 0}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["rows"].as_u64(), Some(3));
    assert_eq!(value["sampled"], false);
    assert_eq!(value["selection"].as_str(), Some("all"));
    let output = value["top"]
        .as_array()
        .expect("top")
        .iter()
        .find(|impact| impact["layer"].as_u64() == Some(1) && impact["row"].as_u64() == Some(0))
        .expect("output row");
    assert_eq!(output["zero"]["changed"].as_u64(), Some(4));
    assert_eq!(output["zero"]["after_loss"].as_u64(), Some(4));
}

#[test]
fn probe_adaptive_pins_output_row() {
    let net = Net::new(vec![
        Layer::new(3, 2, &[0, 0, 1, -1, -1, 1], &[0, 0, 0]).expect("l0"),
        Layer::new(1, 3, &[1, 1, 1], &[0]).expect("l1"),
    ])
    .expect("net");
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_probe",
        json!({
            "artifact": to_wsp(&net),
            "oracle": xor_oracle(),
            "budget": 1,
            "adaptive": true,
            "top": 0,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["selection"].as_str(), Some("adaptive"));
    assert_eq!(value["probed_rows"].as_u64(), Some(1));
    assert_eq!(value["stride"].as_u64(), Some(0));
    let top = &value["top"][0];
    assert_eq!(top["layer"].as_u64(), Some(1));
    assert_eq!(top["row"].as_u64(), Some(0));
}

#[test]
fn localize_returns_row_and_trit() {
    let net = Net::new(vec![
        Layer::new(2, 2, &[1, 1, 0, 0], &[0, 0]).expect("l0"),
        Layer::new(1, 2, &[1, 0], &[0]).expect("l1"),
    ])
    .expect("net");
    let oracle = Oracle::sampled_behavior(&net, 2, 16, 7).expect("oracle");
    let mut broken = net.clone();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut broken)
    .expect("corrupt");

    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_localize",
        json!({
            "artifact": to_wsp(&broken),
            "oracle": trit_wspace::wire::oracle_to_value(&oracle),
            "rows": 2,
            "row_budget": 2,
            "max_trits": 0,
            "top_trits": 4,
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["probe"]["selection"].as_str(), Some("adaptive"));
    let rows = value["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    let row0 = rows
        .iter()
        .find(|row| row["layer"].as_u64() == Some(0) && row["row"].as_u64() == Some(0))
        .expect("row0");
    assert_eq!(row0["selection"].as_str(), Some("all"));
    assert_eq!(row0["top"][0]["after_loss"].as_u64(), Some(0));

    let tight = call(
        &mut server,
        "wsp_localize",
        json!({
            "artifact": to_wsp(&broken),
            "oracle": trit_wspace::wire::oracle_to_value(&oracle),
            "rows": 2,
            "row_budget": 2,
            "max_trits": 2,
            "top_trits": 4,
        }),
    );
    assert_eq!(tight["isError"], false);
    let tight = payload(&tight);
    for row in tight["rows"].as_array().expect("rows") {
        assert_eq!(row["selection"].as_str(), Some("adaptive"));
        assert_eq!(row["stride"].as_u64(), Some(0));
        assert!(row["probed"].as_u64().expect("probed") <= 2);
    }
}

#[test]
fn audit_verifies_chain_and_replays() {
    let start = xor_net();
    let mut broken = start.clone();
    let patch = Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }]);
    patch.apply(&mut broken).expect("corrupt");
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    let mut log = trit_wspace::AuditLog::new();
    log.record_with_oracle("break", &start, &broken, patch, true, false, Some(&oracle))
        .expect("record");

    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_audit",
        json!({"log": log.to_jsonl(), "artifact": to_wsp(&start)}),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["valid"], true);
    assert_eq!(value["entries"].as_u64(), Some(1));
    assert_eq!(value["replay"]["behavior_checked"].as_u64(), Some(1));
    assert_eq!(
        value["replay"]["final_checksum"].as_str().expect("sum"),
        format!("{:016x}", trit_wspace::checksum(&broken))
    );

    let tampered = log.to_jsonl().replacen("\"to\":\"0\"", "\"to\":\"+\"", 1);
    let result = call(&mut server, "wsp_audit", json!({"log": tampered}));
    let value = payload(&result);
    assert_eq!(value["valid"], false);
    assert!(value["error"].is_string());
}

#[test]
fn shard_and_audit_shards_round_trip() {
    let start = xor_net();
    let mut broken = start.clone();
    let patch = Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }]);
    patch.apply(&mut broken).expect("corrupt");
    let mut fixed = broken.clone();
    let fix = Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Neg,
    }]);
    fix.apply(&mut fixed).expect("fix");
    let oracle = Oracle::boolean(2, |bits| bits[0] != bits[1]).expect("oracle");
    let mut log = trit_wspace::AuditLog::new();
    log.record_with_oracle("break", &start, &broken, patch, true, false, Some(&oracle))
        .expect("record break");
    log.record_with_oracle("fix", &broken, &fixed, fix, true, false, Some(&oracle))
        .expect("record fix");

    let mut server = Mcp::new();
    let split = call(
        &mut server,
        "wsp_shard",
        json!({"log": log.to_jsonl(), "size": 1}),
    );
    assert_eq!(split["isError"], false);
    let split = payload(&split);
    assert_eq!(split["valid"], true);
    assert_eq!(split["entries"].as_u64(), Some(2));
    assert_eq!(split["total"].as_u64(), Some(2));
    assert_eq!(split["shards"].as_array().expect("shards").len(), 2);
    assert!(split["manifest"].is_object());
    let manifest = split["manifest"].clone();

    let first = payload(&call(
        &mut server,
        "wsp_shard",
        json!({"log": log.to_jsonl(), "size": 1, "start": 0, "limit": 1}),
    ));
    assert_eq!(first["next"].as_u64(), Some(1));
    let first_shard = first["shards"][0].as_str().expect("first").to_string();
    let second = payload(&call(
        &mut server,
        "wsp_shard",
        json!({"log": log.to_jsonl(), "size": 1, "start": 1, "limit": 1}),
    ));
    assert!(second["next"].is_null());
    let second_shard = second["shards"][0].as_str().expect("second").to_string();

    let checked = payload(&call(
        &mut server,
        "wsp_audit_shards",
        json!({
            "shards": [first_shard, second_shard],
            "manifest": manifest,
            "artifact": to_wsp(&start),
        }),
    ));
    assert_eq!(checked["valid"], true);
    assert_eq!(checked["entries"].as_u64(), Some(2));
    assert_eq!(
        checked["replay"]["final_checksum"].as_str().expect("sum"),
        format!("{:016x}", trit_wspace::checksum(&fixed))
    );

    let mut bad_manifest = manifest;
    bad_manifest["shards"][0]["digest"] = json!("0000000000000000");
    let bad = payload(&call(
        &mut server,
        "wsp_audit_shards",
        json!({"shards": [first_shard, second_shard], "manifest": bad_manifest}),
    ));
    assert_eq!(bad["valid"], false);

    let bad = call(&mut server, "wsp_audit_shards", json!({"shards": [1]}));
    assert_eq!(bad["isError"], true);
}

#[test]
fn diff_and_aligned_merge_are_consistent() {
    let net = xor_net();
    let permuted = net.permute_rows(0, &[1, 0]).expect("permute");
    let a = to_wsp(&net);
    let b = to_wsp(&permuted);
    let mut server = Mcp::new();

    let diff = payload(&call(&mut server, "wsp_diff", json!({"a": a, "b": b})));
    assert!(!diff["patch"]["edits"].as_array().expect("edits").is_empty());

    let merged = payload(&call(
        &mut server,
        "wsp_merge",
        json!({"a": a, "b": b, "aligned": true}),
    ));
    let merged_net = from_wsp(merged["artifact"].as_str().expect("artifact")).expect("parse");
    assert_eq!(merged_net.state_hash(), net.state_hash());

    let naive = payload(&call(&mut server, "wsp_merge", json!({"a": a, "b": b})));
    let naive_net = from_wsp(naive["artifact"].as_str().expect("artifact")).expect("parse");
    assert_ne!(naive_net.state_hash(), net.state_hash());
}

#[test]
fn unknown_tool_and_bad_artifact_are_errors() {
    let mut server = Mcp::new();
    let unknown = call(&mut server, "wsp_nope", json!({}));
    assert_eq!(unknown["isError"], true);
    assert!(text(&unknown).contains("未知工具"));

    let bad = call(&mut server, "wsp_read", json!({"artifact": "not a wsp1"}));
    assert_eq!(bad["isError"], true);

    let bad_depth = call(
        &mut server,
        "wsp_propose",
        json!({"artifact": xor_artifact(), "oracle": xor_oracle(), "depth": 4}),
    );
    assert_eq!(bad_depth["isError"], true);
    assert!(text(&bad_depth).contains("depth"));

    let bad_sample = call(
        &mut server,
        "wsp_explain",
        json!({"artifact": xor_artifact(), "oracle": xor_oracle(), "sample": 99}),
    );
    assert_eq!(bad_sample["isError"], true);
    assert!(text(&bad_sample).contains("越界"));
}

#[test]
fn notifications_have_no_response() {
    let mut server = Mcp::new();
    assert!(
        server
            .dispatch(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .is_none()
    );
    assert!(
        server
            .dispatch(&json!({"jsonrpc": "2.0", "method": "ping"}))
            .is_none()
    );
}

#[test]
fn stdio_loop_answers_one_line_per_request() {
    let input = format!(
        "{}\n{}\n",
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    );
    let mut output = Vec::new();
    trit_wspace::mcp::serve(std::io::Cursor::new(input), &mut output).expect("serve");
    let text = String::from_utf8(output).expect("utf8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("protocolVersion"));
    assert!(lines[1].contains("wsp_read"));
}
