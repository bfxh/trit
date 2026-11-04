//! 线格式不变量：补丁 JSON 往返、验证集紧凑表、失配见证。

use serde_json::json;
use trit_wspace::{Edit, Layer, Net, Trit, wire};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

#[test]
fn patch_json_round_trip_covers_every_edit() {
    let patch = trit_wspace::Patch::new(vec![
        Edit::SetTrit {
            layer: 0,
            row: 1,
            col: 0,
            to: Trit::Zero,
        },
        Edit::SetBias {
            layer: 0,
            row: 0,
            to: Trit::Pos,
        },
        Edit::SetScale {
            layer: 0,
            row: 1,
            to: 2.5,
        },
        Edit::SwapRows {
            layer: 0,
            a: 0,
            b: 1,
        },
        Edit::NegateRow { layer: 0, row: 1 },
    ]);
    let value = wire::patch_to_value(&patch);
    assert_eq!(value["version"].as_u64(), Some(1));
    assert_eq!(value["edits"].as_array().expect("edits").len(), 5);
    assert_eq!(wire::patch_from_value(&value).expect("parse"), patch);
}

#[test]
fn patch_json_rejects_bad_input() {
    for value in [
        json!({}),
        json!({"version": 2, "edits": []}),
        json!({"version": "1", "edits": []}),
        json!({"edits": [{"op": "nope"}]}),
        json!({"edits": [{"op": "set_trit", "layer": 0}]}),
        json!({"edits": [{"op": "set_trit", "layer": 0, "row": 0, "col": 0, "to": "x"}]}),
        json!({"edits": [{"op": "set_scale", "layer": 0, "row": 0, "to": "x"}]}),
    ] {
        assert!(wire::patch_from_value(&value).is_err(), "应当拒绝：{value}");
    }
}

#[test]
fn oracle_boolean_table_matches_exhaustive_order() {
    let oracle = wire::oracle_from_value(&json!({"domain":"boolean","vars":2,"table":"0110"}))
        .expect("oracle");
    assert_eq!(oracle.len(), 4);
    assert_eq!(
        oracle
            .outputs()
            .iter()
            .map(|row| row[0])
            .collect::<Vec<_>>(),
        vec![Trit::Neg, Trit::Pos, Trit::Pos, Trit::Neg]
    );
    assert!(oracle.exact(&xor_net()).expect("exact"));
    assert_eq!(
        oracle.inputs()[0],
        vec![Trit::Neg, Trit::Neg],
        "字典序：最后一个变量变化最快"
    );
}

#[test]
fn oracle_accepts_ternary_and_explicit_rows() {
    let ternary =
        wire::oracle_from_value(&json!({"domain":"ternary","vars":1,"table":"-0+"})).expect("t");
    assert_eq!(ternary.len(), 3);
    assert_eq!(
        ternary
            .outputs()
            .iter()
            .map(|row| row[0])
            .collect::<Vec<_>>(),
        vec![Trit::Neg, Trit::Zero, Trit::Pos]
    );

    let explicit = wire::oracle_from_value(&json!({
        "inputs": [["-","+"], ["+","-"]],
        "outputs": [["+"], ["-"]],
    }))
    .expect("explicit");
    let compact = wire::oracle_from_value(&json!({
        "inputs": ["-+", "+-"],
        "outputs": ["+", "-"],
    }))
    .expect("compact");
    assert_eq!(compact.inputs(), explicit.inputs());
    assert_eq!(compact.outputs(), explicit.outputs());
}

#[test]
fn oracle_rejects_wrong_table_length_and_bad_chars() {
    assert!(wire::oracle_from_value(&json!({"domain":"boolean","vars":2,"table":"011"})).is_err());
    assert!(wire::oracle_from_value(&json!({"domain":"boolean","vars":2,"table":"01-0"})).is_err());
    assert!(wire::oracle_from_value(&json!({"domain":"nope","vars":1,"table":"0"})).is_err());
    assert!(wire::oracle_from_value(&json!({"vars":1,"table":"0"})).is_err());
}

#[test]
fn mismatches_carry_input_want_and_got() {
    let mut net = xor_net();
    trit_wspace::Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut net)
    .expect("corrupt");
    let oracle = wire::oracle_from_value(&json!({"domain":"boolean","vars":2,"table":"0110"}))
        .expect("oracle");
    let value = wire::mismatches_value(&net, &oracle, 8).expect("mismatches");
    let count = value["count"].as_u64().expect("count");
    assert!(count >= 1);
    let items = value["items"].as_array().expect("items");
    assert_eq!(items.len() as u64, count);
    assert!(items[0]["input"].is_string());
    assert!(items[0]["want"].is_string());
    assert!(items[0]["got"].is_string());
    assert_eq!(value["truncated"].as_bool(), Some(false));
}

#[test]
fn summary_exposes_arch_count_and_checksums() {
    let net = xor_net();
    let summary = wire::net_summary(&net);
    assert_eq!(summary["arch"], json!([2, 2, 1]));
    assert_eq!(summary["depth"].as_u64(), Some(2));
    assert_eq!(summary["trits"].as_u64(), Some(9));
    assert_eq!(
        summary["checksum"],
        json!(format!("{:016x}", trit_wspace::checksum(&net)))
    );
    assert_eq!(
        summary["state"],
        json!(format!("{:016x}", net.state_hash()))
    );
}
