//! 置换密钥：行为不变、恒等无操作、不同种子不同排列、对齐攻击读数、JSON 形状。

use serde_json::json;
use trit_wspace::perm_key::{self, AttackReport, PermKey};
use trit_wspace::{Layer, Net, Oracle, to_wsp};

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
fn permuting_preserves_behavior() {
    let net = xor_net();
    let oracle = xor_oracle();
    let key = perm_key::from_seed(&net, 7);
    let permuted = perm_key::apply(&net, &key).expect("apply");
    assert_eq!(
        oracle.outputs_of(&net).expect("base"),
        oracle.outputs_of(&permuted).expect("permuted"),
        "置换不应改变行为"
    );
}

#[test]
fn identity_key_is_a_noop() {
    let net = xor_net();
    let key = perm_key::identity(&net);
    let applied = perm_key::apply(&net, &key).expect("apply");
    assert_eq!(to_wsp(&applied), to_wsp(&net));
}

#[test]
fn different_seeds_give_different_keys() {
    let net = xor_net();
    let left = perm_key::from_seed(&net, 1);
    let right = perm_key::from_seed(&net, 2);
    assert_ne!(
        left, right,
        "不同种子应产生不同排列（2 行时 50% 概率相同，换种子仍可能相等）"
    );
}

#[test]
fn attack_reports_match_rate() {
    let net = xor_net();
    let oracle = xor_oracle();
    let key = perm_key::from_seed(&net, 7);
    let permuted = perm_key::apply(&net, &key).expect("apply");
    let report = perm_key::attack(&net, &permuted, &oracle).expect("attack");
    assert!(report.behavior_identical, "置换后行为必须与 base 相同");
    assert_eq!(report.trits, 6);
    assert!(report.aligned_matches <= report.trits);
    assert!((0.0..=1.0).contains(&report.match_rate()));
}

#[test]
fn wrong_key_shape_is_rejected() {
    let net = xor_net();
    let key = PermKey {
        layers: vec![vec![0, 1], vec![0]],
    };
    let error = perm_key::apply(&net, &key).expect_err("层数不符必须报错");
    assert!(format!("{error}").contains("隐藏层"), "{error}");
}

#[test]
fn json_has_all_fields() {
    let report = AttackReport {
        layers: 2,
        trits: 6,
        aligned_matches: 6,
        attack_success: true,
        behavior_identical: true,
    };
    let value = report.to_json();
    assert_eq!(value["schema"], "trit-wspace.perm-attack.v1");
    assert_eq!(value["match_rate"], 1.0);
    assert_eq!(value["attack_success"], true);
}
