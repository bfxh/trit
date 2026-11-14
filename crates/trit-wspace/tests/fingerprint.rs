//! 挑战集行为指纹：往返、失配定位、摘要校验、样本数不一致。

use serde_json::json;
use trit_wspace::fingerprint::{self, Fingerprint};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit};

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
fn emit_then_check_passes() {
    let net = xor_net();
    let oracle = xor_oracle();
    let saved = fingerprint::emit(&net, &oracle).expect("emit");
    assert_eq!(saved.samples, oracle.len());
    let report = fingerprint::check(&net, &oracle, &saved, 16).expect("check");
    assert!(report.passed(), "{report:?}");
    assert_eq!(report.matched, report.samples);
    assert!(report.mismatched.is_empty());
}

#[test]
fn edited_artifact_breaks_the_fingerprint() {
    let net = xor_net();
    let oracle = xor_oracle();
    let saved = fingerprint::emit(&net, &oracle).expect("emit");
    let mut edited = net.clone();
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Pos,
    }])
    .apply(&mut edited)
    .expect("edit");
    let report = fingerprint::check(&edited, &oracle, &saved, 16).expect("check");
    assert!(!report.passed());
    assert!(!report.digest_match, "摘要应变化");
    assert!(!report.mismatched.is_empty(), "应有失配下标");
}

#[test]
fn json_round_trip_is_exact() {
    let saved = fingerprint::emit(&xor_net(), &xor_oracle()).expect("emit");
    let value = saved.to_json();
    let back = Fingerprint::from_json(&value).expect("from_json");
    assert_eq!(back, saved);
}

#[test]
fn tampered_digest_is_rejected() {
    let saved = fingerprint::emit(&xor_net(), &xor_oracle()).expect("emit");
    let mut value = saved.to_json();
    value["digest"] = json!("0000000000000000");
    let error = Fingerprint::from_json(&value).expect_err("摘要不符必须报错");
    assert!(format!("{error}").contains("摘要不符"), "{error}");
}

#[test]
fn sample_count_mismatch_is_rejected() {
    let net = xor_net();
    let saved = fingerprint::emit(&net, &xor_oracle()).expect("emit");
    let other = trit_wspace::wire::oracle_from_value(&json!({
        "inputs": [["-", "-"], ["-", "+"], ["+", "-"]],
        "outputs": [["-"], ["+"], ["+"]]
    }))
    .expect("oracle");
    let error = fingerprint::check(&net, &other, &saved, 16).expect_err("样本数不一致必须报错");
    assert!(format!("{error}").contains("样本数不一致"), "{error}");
}
