//! 审计日志幂等：同一条编辑重复记录只留一条；不同条目仍追加。

use serde_json::json;
use trit_wspace::{AuditLog, Edit, Layer, Net, Oracle, Patch, Trit};

fn xor_net() -> Net {
    Net::new(vec![
        Layer::new(2, 2, &[-1, -1, -1, -1], &[-1, 1]).expect("l0"),
        Layer::new(1, 2, &[-1, 1], &[-1]).expect("l1"),
    ])
    .expect("xor net")
}

fn break_patch() -> Patch {
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
}

fn xor_oracle() -> Oracle {
    trit_wspace::wire::oracle_from_value(&json!({
        "domain": "boolean", "vars": 2, "table": "0110"
    }))
    .expect("oracle")
}

#[test]
fn identical_record_is_skipped() {
    let start = xor_net();
    let mut broken = start.clone();
    let patch = break_patch();
    patch.apply(&mut broken).expect("apply");
    let mut log = AuditLog::new();
    log.record("break", &start, &broken, patch.clone(), true, false)
        .expect("first");
    assert_eq!(log.len(), 1);
    log.record("break", &start, &broken, patch.clone(), true, false)
        .expect("repeat");
    assert_eq!(log.len(), 1, "重复应用不应产生重复条");
    log.verify_chain().expect("链仍自洽");
}

#[test]
fn different_name_is_not_skipped() {
    let start = xor_net();
    let mut broken = start.clone();
    let patch = break_patch();
    patch.apply(&mut broken).expect("apply");
    let mut log = AuditLog::new();
    log.record("break", &start, &broken, patch.clone(), true, false)
        .expect("first");
    log.record("break-again", &start, &broken, patch, true, false)
        .expect("second");
    assert_eq!(log.len(), 2, "名字不同是另一条历史");
}

#[test]
fn duplicate_with_oracle_is_skipped() {
    let start = xor_net();
    let mut broken = start.clone();
    let patch = break_patch();
    patch.apply(&mut broken).expect("apply");
    let oracle = xor_oracle();
    let mut log = AuditLog::new();
    log.record_with_oracle(
        "break",
        &start,
        &broken,
        patch.clone(),
        true,
        false,
        Some(&oracle),
    )
    .expect("first");
    log.record_with_oracle("break", &start, &broken, patch, true, false, Some(&oracle))
        .expect("repeat");
    assert_eq!(log.len(), 1, "带验证集的重复同样只留一条");
}

#[test]
fn applied_flag_difference_is_not_skipped() {
    let start = xor_net();
    let mut broken = start.clone();
    let patch = break_patch();
    patch.apply(&mut broken).expect("apply");
    let mut log = AuditLog::new();
    log.record("x", &start, &broken, patch.clone(), true, false)
        .expect("applied");
    log.record("x", &start, &start.clone(), patch, false, false)
        .expect("blocked");
    assert_eq!(log.len(), 2, "applied 不同 ⇒ 不是重复");
}

#[test]
fn duplicate_after_intermediate_edit_is_kept() {
    // 中间插入了另一条编辑 ⇒ 工件状态已前进，再记一次是合法历史。
    let start = xor_net();
    let mut broken = start.clone();
    let patch = break_patch();
    patch.apply(&mut broken).expect("apply");
    let mut fixed = broken.clone();
    let fix = Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Neg,
    }]);
    fix.apply(&mut fixed).expect("fix");
    let mut log = AuditLog::new();
    log.record("break", &start, &broken, patch.clone(), true, false)
        .expect("break");
    log.record("fix", &broken, &fixed, fix, true, false)
        .expect("fix");
    log.record("break", &start, &broken, patch, true, false)
        .expect("break again");
    assert_eq!(log.len(), 3, "中间有别的编辑时应保留");
}
