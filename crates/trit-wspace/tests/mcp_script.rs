//! MCP 脚本工具：组合脚本执行闭环。

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{Layer, Net, from_wsp, to_wsp};

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

fn call(server: &mut Mcp, name: &str, args: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": name, "arguments": args},
    });
    let response = server.dispatch(&request).expect("response");
    response["result"].clone()
}

fn payload(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().expect("text content"))
        .expect("json payload")
}

#[test]
fn run_script_executes_sequence() {
    let script = json!({
        "type": "sequence",
        "name": "repair",
        "steps": [
            {"type": "step", "name": "break", "patch": {"version": 1, "edits": [
                {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "0"}]}},
            {"type": "step", "name": "fix", "patch": {"version": 1, "edits": [
                {"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "-"}]}}
        ]
    });
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_run_script",
        json!({
            "artifact": xor_artifact(),
            "script": script,
            "oracle": {"domain": "boolean", "vars": 2, "table": "0110"},
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["report"]["type"], "sequence");
    assert_eq!(value["report"]["rolled_back"], false);
    assert_eq!(value["report"]["steps"].as_array().expect("steps").len(), 2);
    let net = from_wsp(value["artifact"].as_str().expect("artifact")).expect("parse");
    assert_eq!(net, xor_net());
}
