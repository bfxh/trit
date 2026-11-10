//! MCP 任务向量合并工具。

use serde_json::{Value, json};
use trit_wspace::mcp::Mcp;
use trit_wspace::{Edit, Layer, Net, Patch, Trit, from_wsp, to_wsp};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn tuned(edits: Vec<Edit>) -> Net {
    let mut net = xor_net();
    Patch::new(edits).apply(&mut net).expect("tune");
    net
}

fn call(server: &mut Mcp, name: &str, args: Value) -> Value {
    server
        .dispatch(&json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": name, "arguments": args},
        }))
        .expect("response")["result"]
        .clone()
}

fn payload(result: &Value) -> Value {
    let raw = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(raw).expect("json payload")
}

#[test]
fn task_merge_via_mcp() {
    let base = xor_net();
    let left = tuned(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }]);
    let right = tuned(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 1,
        to: Trit::Zero,
    }]);
    let mut server = Mcp::new();
    let result = call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&base),
            "tasks": [to_wsp(&left), to_wsp(&right)],
        }),
    );
    assert_eq!(result["isError"], false);
    let value = payload(&result);
    assert_eq!(value["tasks"].as_u64(), Some(2));
    assert_eq!(value["cost"]["trits"].as_u64(), Some(2));
    let merged = from_wsp(value["artifact"].as_str().expect("artifact")).expect("parse");
    let layer = merged.layer(0).expect("layer");
    assert_eq!(layer.trit(0, 0), Some(Trit::Zero));
    assert_eq!(layer.trit(0, 1), Some(Trit::Zero));

    let permuted = xor_net().permute_rows(0, &[1, 0]).expect("permute");
    let aligned = payload(&call(
        &mut server,
        "wsp_task_merge",
        json!({
            "base": to_wsp(&xor_net()),
            "tasks": [to_wsp(&permuted)],
            "aligned": true,
        }),
    ));
    assert_eq!(aligned["cost"]["trits"].as_u64(), Some(0));
    let net = from_wsp(aligned["artifact"].as_str().expect("artifact")).expect("parse");
    assert_eq!(net, xor_net());
}
