//! MCP 工具面：协议、六个工具、回滚与错误。

use serde_json::{Value, json};
use trit_wspace::mcp::{Mcp, PROTOCOL_VERSION, tools};
use trit_wspace::{Edit, Layer, Net, Patch, Trit, from_wsp, to_wsp};

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
fn lists_exactly_six_tools() {
    let mut server = Mcp::new();
    let response = server
        .dispatch(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .expect("response");
    let listed = response["result"]["tools"].as_array().expect("tools");
    assert_eq!(listed.len(), 6);
    let names: Vec<&str> = listed
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    for expected in [
        "wsp_read",
        "wsp_verify",
        "wsp_patch",
        "wsp_repair",
        "wsp_diff",
        "wsp_merge",
    ] {
        assert!(names.contains(&expected), "缺少工具 {expected}");
    }
    assert_eq!(tools().len(), 6);
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
