//! MCP 分片工具：manifest、分页、检查点与续放。

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit, to_wsp};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn call(server: &mut Mcp, name: &str, args: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": name, "arguments": args},
    });
    let response = server.dispatch(&request).expect("response");
    response["result"].clone()
}

fn payload(result: &Value) -> Value {
    let raw = result["content"][0]["text"]
        .as_str()
        .expect("text content")
        .to_string();
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("payload: {error}; raw={raw}"))
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

    let raw = call(
        &mut server,
        "wsp_audit_shards",
        json!({
            "shards": all_shards(&first_shard, &second_shard),
            "artifact": to_wsp(&start),
            "checkpoint_every": 1,
        }),
    );
    assert_eq!(raw["isError"], false, "{raw}");
    let checkpointed = payload(&raw);
    assert_eq!(checkpointed["valid"], true);
    let checkpoints = checkpointed["checkpoints"].as_array().expect("checkpoints");
    assert_eq!(checkpoints.len(), 2);
    let checkpoint = checkpoints[0].clone();
    assert_eq!(checkpoint["shard"].as_u64(), Some(1));

    let raw = call(
        &mut server,
        "wsp_audit_shards",
        json!({
            "shards": all_shards(&first_shard, &second_shard),
            "checkpoint": checkpoint,
        }),
    );
    assert_eq!(raw["isError"], false, "{raw}");
    let resumed = payload(&raw);
    assert_eq!(resumed["valid"], true);
    assert_eq!(resumed["resumed"], true);
    assert_eq!(resumed["replay"]["entries"].as_u64(), Some(2));
    assert_eq!(
        resumed["replay"]["final_checksum"],
        checkpointed["replay"]["final_checksum"]
    );

    let mut bad_checkpoint = checkpoint;
    bad_checkpoint["head"] = json!("0000000000000000");
    let bad = payload(&call(
        &mut server,
        "wsp_audit_shards",
        json!({
            "shards": all_shards(&first_shard, &second_shard),
            "checkpoint": bad_checkpoint,
        }),
    ));
    assert_eq!(bad["valid"], false);

    let bad = call(&mut server, "wsp_audit_shards", json!({"shards": [1]}));
    assert_eq!(bad["isError"], true);
}

fn all_shards(first: &str, second: &str) -> Vec<String> {
    vec![first.to_string(), second.to_string()]
}
