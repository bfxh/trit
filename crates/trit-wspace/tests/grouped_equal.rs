//! "scale 无关"判等的三个机制：行为签名、规范化、per-row 判定。

use trit_wspace::grouped::GroupedLayer;
use trit_wspace::grouped_equal::MAX_VARS;

fn layer(trits: &[i8], groups: &[u32], scales: &[f32]) -> GroupedLayer {
    GroupedLayer::from_trits(2, 2, trits, groups, scales, &[0.0, 0.0]).expect("layer")
}

#[test]
fn behavior_ignores_overall_scale() {
    // 全体 scale 同乘一个正数不改变符号行为 ⇒ 行为判等应返回 true。
    let a = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[1.0, 2.0]);
    let b = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[3.0, 6.0]);
    assert!(
        a.equivalent(&b, 2).expect("equivalent"),
        "整体缩放不应改变行为"
    );
    assert_eq!(a.behavior(2).expect("a"), b.behavior(2).expect("b"));
}

#[test]
fn behavior_detects_scale_swap() {
    // 把两组的 scale 对调会改变行内相对权重 ⇒ 行为可以不同。
    let a = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[0.1, 10.0]);
    let b = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[10.0, 0.1]);
    let same = a.equivalent(&b, 2).expect("equivalent");
    assert!(!same, "对调组 scale 应当改变行为（若相同说明测试构造失效）");
}

#[test]
fn canonical_normalizes_first_scale_and_keeps_behavior() {
    let a = layer(&[1, -1, 0, 1], &[0, 0, 1, 1], &[2.0, 8.0]);
    let c = a.canonical();
    assert!(
        (c.scales()[0] - 1.0).abs() < 1e-6,
        "第一个 scale 应归一为 1"
    );
    assert!((c.scales()[1] - 4.0).abs() < 1e-6, "比例应保持 1:4");
    assert!(a.equivalent(&c, 2).expect("eq"), "规范化不应改变行为");
}

#[test]
fn canonical_forms_of_scaled_copies_agree() {
    // 整体缩放的两份，规范化后应逐项相同 ⇒ 可作为"快速判等"的规范形式。
    let a = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[1.0, 2.0]);
    let b = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[3.0, 6.0]);
    assert_eq!(a.canonical(), b.canonical());
}

#[test]
fn row_grouped_detection() {
    // 每行自成一组（组 0 只含第 0 行，组 1 只含第 1 行）
    let per_row = layer(&[1, 1, 1, -1], &[0, 0, 1, 1], &[1.0, 2.0]);
    assert!(per_row.is_row_grouped(), "per-row 分组应被识别");

    // 跨行分组（组 0/1 同时横跨两行）
    let cross = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[1.0, 2.0]);
    assert!(!cross.is_row_grouped(), "跨行分组不应被识别为 per-row");
}

#[test]
fn behavior_rejects_too_many_vars() {
    let a = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[1.0, 2.0]);
    assert!(a.behavior(MAX_VARS + 1).is_err(), "超过上限应报错");
    // 4 列却要 5 个变量
    assert!(a.behavior(5).is_err(), "vars 超过列数应报错");
}

#[test]
fn shape_mismatch_is_not_equivalent() {
    let a = GroupedLayer::from_trits(2, 2, &[0; 4], &[0, 0, 1, 1], &[1.0, 1.0], &[0.0, 0.0])
        .expect("a");
    let b = GroupedLayer::from_trits(1, 4, &[0; 4], &[0, 0, 0, 0], &[1.0], &[0.0]).expect("b");
    assert!(!a.equivalent(&b, 2).expect("eq"), "形状不同应直接判不等");
}
