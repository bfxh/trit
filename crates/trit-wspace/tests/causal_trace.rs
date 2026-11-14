//! 激活级因果追踪：corrupt 全域后输出变差、逐层/逐单元 restore 的单调性与形状。

use serde_json::json;
use trit_wspace::causal_trace::causal_trace;
use trit_wspace::{Layer, Net, Oracle};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn xor_oracle() -> Oracle {
    trit_wspace::wire::oracle_from_value(&json!({
        "domain": "boolean", "vars": 2, "table": "0110"
    }))
    .expect("oracle")
}

#[test]
fn corrupting_everything_hurts_and_restoring_recovers() {
    let trace = causal_trace(&xor_net(), &xor_oracle(), 0).expect("trace");
    let intact = trace
        .intact
        .iter()
        .zip(&trace.target)
        .filter(|(a, b)| a == b)
        .count();
    let corrupted = trace
        .corrupted
        .iter()
        .zip(&trace.target)
        .filter(|(a, b)| a == b)
        .count();
    assert!(corrupted <= intact, "corrupt 不应提升匹配位数");
    let best = trace
        .layers
        .iter()
        .map(|layer| layer.matches)
        .max()
        .unwrap_or(0);
    assert!(best > corrupted, "至少有一层单独 restore 能恢复一部分");
}

#[test]
fn restore_curve_covers_every_unit() {
    let net = xor_net();
    let trace = causal_trace(&net, &xor_oracle(), 0).expect("trace");
    let expected: usize = net.layers().iter().map(Layer::rows).sum();
    assert_eq!(trace.restore_curve.len(), expected);
    let mut seen = std::collections::BTreeSet::new();
    for step in &trace.restore_curve {
        assert!(seen.insert((step.layer, step.row)), "单元被重复恢复");
    }
    assert_eq!(seen.len(), expected);
}

#[test]
fn restore_curve_matches_are_monotone() {
    let trace = causal_trace(&xor_net(), &xor_oracle(), 0).expect("trace");
    let mut previous = 0;
    for step in &trace.restore_curve {
        assert!(
            step.matches >= previous,
            "累计恢复的匹配位数不应下降：{} -> {}",
            previous,
            step.matches
        );
        previous = step.matches;
    }
}

#[test]
fn sample_out_of_range_is_an_error() {
    let error = causal_trace(&xor_net(), &xor_oracle(), 99).expect_err("必须报错");
    assert!(format!("{error}").contains("越界"), "{error}");
}

#[test]
fn json_has_all_fields() {
    let trace = causal_trace(&xor_net(), &xor_oracle(), 0).expect("trace");
    let value = trace.to_json();
    assert_eq!(value["schema"], "trit-wspace.causal-trace.v1");
    assert!(value["target"].is_array());
    assert_eq!(
        value["layers"].as_array().expect("layers").len(),
        xor_net().layers().len()
    );
    assert!(!value["restore_curve"].as_array().expect("curve").is_empty());
}
