//! 连续编辑协议：空补丁落地、破坏历史回滚、allow 容忍、edit 未达成回滚、JSON 形状。

use serde_json::json;
use trit_wspace::sequence::{SequenceStep, run_sequence};
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit, to_wsp};

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

fn break_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
}

fn empty_patch() -> Patch {
    Patch::new(Vec::new())
}

fn step(name: &str, patch: Patch, keep: Vec<Oracle>, edit: Option<Oracle>) -> SequenceStep {
    SequenceStep {
        name: name.to_string(),
        patch,
        keep,
        edit,
    }
}

#[test]
fn harmless_steps_are_applied() {
    let base = xor_net();
    let oracle = xor_oracle();
    let steps = vec![
        step("noop-1", empty_patch(), vec![oracle.clone()], None),
        step("noop-2", empty_patch(), vec![oracle.clone()], None),
    ];
    let outcome = run_sequence(&base, &steps, 0).expect("run");
    assert_eq!(outcome.applied, 2);
    assert_eq!(outcome.rolled_back, 0);
    assert_eq!(to_wsp(&outcome.final_net), to_wsp(&base));
}

#[test]
fn broken_history_rolls_back_to_previous_step() {
    let base = xor_net();
    let oracle = xor_oracle();
    let steps = vec![
        step("noop", empty_patch(), vec![oracle.clone()], None),
        step("break", break_patch(), vec![oracle.clone()], None),
    ];
    let outcome = run_sequence(&base, &steps, 0).expect("run");
    assert!(outcome.steps[0].applied);
    assert!(!outcome.steps[1].applied);
    assert!(outcome.steps[1].rolled_back);
    assert_eq!(outcome.rolled_back, 1);
    assert_eq!(
        to_wsp(&outcome.final_net),
        to_wsp(&base),
        "回滚后应停在上一步（这里是 base）"
    );
}

#[test]
fn allow_tolerates_known_violations() {
    let base = xor_net();
    let oracle = xor_oracle();
    let broken = {
        let mut trial = base.clone();
        break_patch().apply(&mut trial).expect("apply");
        trial
    };
    let violation = oracle.loss(&broken).expect("loss");
    // 破坏后的失配数就是 allow 的临界值：allow = violation 时应通过
    let steps = vec![step("break", break_patch(), vec![oracle], None)];
    let passed = run_sequence(&base, &steps, violation).expect("run");
    assert!(passed.steps[0].applied, "allow = {violation} 应容忍");
    let blocked = run_sequence(&base, &steps, violation.saturating_sub(1)).expect("run");
    if violation > 0 {
        assert!(!blocked.steps[0].applied, "allow 小于失配数时应回滚");
    }
}

#[test]
fn unmet_edit_rolls_back() {
    let base = xor_net();
    let oracle = xor_oracle();
    // 编辑集要求一个与当前输出完全不同的签名：必然达不成
    let impossible = trit_wspace::wire::oracle_from_value(&json!({
        "inputs": [["-", "-"]], "outputs": [["+"]]
    }))
    .expect("oracle");
    let current = oracle.outputs_of(&base).expect("outputs");
    let impossible = if current[0][0] == Trit::Pos {
        trit_wspace::wire::oracle_from_value(&json!({
            "inputs": [["-", "-"]], "outputs": [["-"]]
        }))
        .expect("oracle")
    } else {
        impossible
    };
    let steps = vec![step("noop", empty_patch(), Vec::new(), Some(impossible))];
    let outcome = run_sequence(&base, &steps, 0).expect("run");
    assert!(!outcome.steps[0].applied, "edit 未达成必须回滚");
    assert_eq!(outcome.rolled_back, 1);
}

#[test]
fn json_has_all_fields() {
    let base = xor_net();
    let oracle = xor_oracle();
    let steps = vec![step("noop", empty_patch(), vec![oracle], None)];
    let outcome = run_sequence(&base, &steps, 0).expect("run");
    let value = outcome.to_json();
    assert_eq!(value["schema"], "trit-wspace.sequence.v1");
    assert_eq!(value["applied"], 1);
    assert!(value["steps"].as_array().expect("steps")[0]["applied"].is_boolean());
    assert!(value["final_checksum"].is_string());
}
