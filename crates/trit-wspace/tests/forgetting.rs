//! 序列编辑遗忘曲线：逐步应用、历史保持率、JSON 形状。

use serde_json::json;
use trit_wspace::forgetting::{ForgettingInput, forgetting_curve};
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

fn break_patch(to: Trit) -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to,
    }])
}

#[test]
fn empty_history_keeps_full_retention() {
    let curve = forgetting_curve(
        &xor_net(),
        &[ForgettingInput {
            patch: break_patch(Trit::Zero),
            own: None,
        }],
        &[],
    )
    .expect("curve");
    assert_eq!(curve.oracles(), 0);
    assert_eq!(curve.steps.len(), 1);
    assert!((curve.steps[0].mean_retention - 1.0).abs() < 1e-12);
    assert!(curve.final_retention.is_empty());
}

#[test]
fn retention_drops_when_history_is_damaged() {
    let oracle = xor_oracle();
    let base_retention = {
        let curve = forgetting_curve(&xor_net(), &[], std::slice::from_ref(&oracle)).expect("base");
        curve.base[0]
    };
    // 连续两步都改同一个承重 trit：历史任务的保持率应单调不增
    let curve = forgetting_curve(
        &xor_net(),
        &[
            ForgettingInput {
                patch: break_patch(Trit::Zero),
                own: Some(oracle.clone()),
            },
            ForgettingInput {
                patch: break_patch(Trit::Pos),
                own: Some(oracle.clone()),
            },
        ],
        std::slice::from_ref(&oracle),
    )
    .expect("curve");
    assert_eq!(curve.steps.len(), 2);
    let first = curve.steps[0].retention[0];
    let second = curve.steps[1].retention[0];
    assert!(
        first <= base_retention + 1e-12,
        "第一步不应提升历史保持率：{first} vs {base_retention}"
    );
    assert!(
        second <= first + 1e-12,
        "第二步不应提升历史保持率：{second} vs {first}"
    );
    assert_eq!(curve.final_retention, curve.steps[1].retention);
}

#[test]
fn own_loss_is_recorded() {
    let oracle = xor_oracle();
    let curve = forgetting_curve(
        &xor_net(),
        &[ForgettingInput {
            patch: break_patch(Trit::Zero),
            own: Some(oracle.clone()),
        }],
        std::slice::from_ref(&oracle),
    )
    .expect("curve");
    assert!(curve.steps[0].loss.is_some());
}

#[test]
fn json_has_matrix_shape() {
    let oracle = xor_oracle();
    let curve = forgetting_curve(
        &xor_net(),
        &[ForgettingInput {
            patch: break_patch(Trit::Zero),
            own: None,
        }],
        &[oracle.clone(), oracle],
    )
    .expect("curve");
    let value = curve.to_json();
    assert_eq!(value["schema"], "trit-wspace.forgetting.v1");
    assert_eq!(value["oracles"], 2);
    assert_eq!(value["base"].as_array().expect("base").len(), 2);
    assert_eq!(
        value["steps"][0]["retention"]
            .as_array()
            .expect("row")
            .len(),
        2
    );
    assert_eq!(value["final_retention"].as_array().expect("final").len(), 2);
}
