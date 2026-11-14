//! 零活跃 trit 水印：候选筛选、嵌入后行为不变、精确提取、容量不足报错。

use serde_json::json;
use trit_wspace::watermark::{embed, extract, find_positions};
use trit_wspace::{Layer, Net, Oracle};

/// 下游权重全 0 ⇒ 第一层的任何 trit 改动都不影响输出，且第一层 8 个 trit 全为 0。
fn dead_net() -> Net {
    Net::new(vec![
        Layer::new(4, 2, &[0; 8], &[-1, 1, -1, 1]).expect("l0"),
        Layer::new(1, 4, &[0, 0, 0, 0], &[1]).expect("l1"),
    ])
    .expect("dead net")
}

fn xor_oracle() -> Oracle {
    trit_wspace::wire::oracle_from_value(&json!({
        "domain": "boolean", "vars": 2, "table": "0110"
    }))
    .expect("oracle")
}

#[test]
fn finds_zero_active_positions() {
    let net = dead_net();
    let oracle = xor_oracle();
    let base_loss = oracle.loss(&net).expect("loss");
    let positions = find_positions(&net, &oracle, base_loss, 8, 100).expect("positions");
    assert!(!positions.is_empty());
    for (layer, row, col) in &positions {
        assert_eq!(
            net.layers()[*layer].trit(*row, *col),
            Some(trit_wspace::Trit::Zero)
        );
    }
}

#[test]
fn embed_keeps_behavior_and_extract_round_trips() {
    let net = dead_net();
    let oracle = xor_oracle();
    let message = [true, false, true, true, false, false, true, false];
    let (edited, report) = embed(&net, &oracle, &message, 7, 100).expect("embed");
    assert!(report.verified(), "嵌入前后 loss 必须相同");
    assert_eq!(report.bits, message);
    assert_eq!(report.positions.len(), message.len());
    let bits = extract(&edited, &report.positions);
    assert_eq!(bits, message.to_vec());
}

#[test]
fn capacity_shortage_is_an_error() {
    let net = dead_net();
    let oracle = xor_oracle();
    let message = vec![true; 32];
    let error = embed(&net, &oracle, &message, 7, 100).expect_err("容量不足必须报错");
    assert!(format!("{error}").contains("容量不足"), "{error}");
}

#[test]
fn empty_message_is_a_noop() {
    let net = dead_net();
    let oracle = xor_oracle();
    let (edited, report) = embed(&net, &oracle, &[], 7, 100).expect("embed");
    assert!(report.positions.is_empty());
    assert!(report.verified());
    assert_eq!(
        trit_wspace::to_wsp(&edited),
        trit_wspace::to_wsp(&net),
        "空消息不应改动工件"
    );
}

#[test]
fn json_has_all_fields() {
    let net = dead_net();
    let oracle = xor_oracle();
    let (_, report) = embed(&net, &oracle, &[true, false], 7, 100).expect("embed");
    let value = report.to_json();
    assert_eq!(value["schema"], "trit-wspace.watermark.v1");
    assert_eq!(value["bits"], 2);
    assert_eq!(value["verified"], true);
    assert_eq!(value["positions"].as_array().expect("positions").len(), 2);
}
