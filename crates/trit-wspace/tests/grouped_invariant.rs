//! 更粗粒度的判等不变量：逐行正比例。

use trit_wspace::grouped::GroupedLayer;

fn layer(trits: &[i8], groups: &[u32], scales: &[f32]) -> GroupedLayer {
    GroupedLayer::from_trits(2, 3, trits, groups, scales, &[0.0, 0.0]).expect("layer")
}

#[test]
fn global_scale_is_proportional() {
    // 整体缩放：每行比例相同
    let a = layer(&[1, 1, -1, 1, -1, 1], &[0, 0, 0, 1, 1, 1], &[1.0, 2.0]);
    let b = layer(&[1, 1, -1, 1, -1, 1], &[0, 0, 0, 1, 1, 1], &[3.0, 6.0]);
    let ratios = a.row_proportional(&b).expect("应成比例");
    assert_eq!(ratios.len(), 2);
    for ratio in &ratios {
        assert!(
            (ratio - 3.0).abs() < 1e-6,
            "整体缩放比例应为 3，实际 {ratios:?}"
        );
    }
    assert!(a.globally_proportional(&b), "整体缩放应被识别");
}

#[test]
fn per_row_scale_is_proportional_but_not_global() {
    // 每行一个 scale（Layer 语义）：逐行成比例，但比例不同 ⇒ 不是"整体"缩放
    let a = layer(&[1, 1, -1, 1, -1, 1], &[0, 0, 0, 1, 1, 1], &[1.0, 1.0]);
    let b = layer(&[1, 1, -1, 1, -1, 1], &[0, 0, 0, 1, 1, 1], &[2.0, 5.0]);
    let ratios = a.row_proportional(&b).expect("逐行应成比例");
    assert!((ratios[0] - 2.0).abs() < 1e-6);
    assert!((ratios[1] - 5.0).abs() < 1e-6);
    assert!(
        !a.globally_proportional(&b),
        "逐行比例不同时不应判为整体缩放"
    );
}

#[test]
fn cross_row_group_swap_breaks_proportional() {
    // 跨行分组：把两组的 scale 对调，任何行内都混着两个比例 ⇒ 不成比例
    let a = layer(&[1, 1, -1, 1, -1, 1], &[0, 1, 0, 1, 0, 1], &[1.0, 2.0]);
    let b = layer(&[1, 1, -1, 1, -1, 1], &[0, 1, 0, 1, 0, 1], &[2.0, 1.0]);
    assert!(
        a.row_proportional(&b).is_none(),
        "跨行分组对调 scale 后不应逐行成比例"
    );
}

#[test]
fn proportional_implies_same_behavior() {
    // 充分条件的性质测试：比例存在 ⇒ 符号行为一致
    let base = layer(&[1, 0, -1, 1, -1, 0], &[0, 0, 0, 1, 1, 1], &[1.0, 2.0]);
    for k0 in [0.5f32, 2.0, 7.5] {
        for k1 in [0.25f32, 3.0, 11.0] {
            let scaled = GroupedLayer::from_trits(
                2,
                3,
                &[1, 0, -1, 1, -1, 0],
                &[0, 0, 0, 1, 1, 1],
                &[k0, k1],
                &[0.0, 0.0],
            )
            .expect("scaled");
            if base.row_proportional(&scaled).is_some() {
                assert!(
                    base.equivalent(&scaled, 3).expect("eq"),
                    "逐行成比例时行为必须相同（k0={k0}, k1={k1}）"
                );
            }
        }
    }
}

#[test]
fn zero_cannot_scale_to_nonzero() {
    // a 为 0、b 非 0 ⇒ 无法用比例表达
    let a = layer(&[1, 0, -1, 1, -1, 0], &[0, 0, 0, 1, 1, 1], &[1.0, 2.0]);
    let b = layer(&[1, 1, -1, 1, -1, 0], &[0, 0, 0, 1, 1, 1], &[1.0, 2.0]);
    assert!(a.row_proportional(&b).is_none(), "零无法按比例变成非零");
}

#[test]
fn negative_ratio_is_rejected() {
    // 负比例会翻转符号 ⇒ 必须拒绝
    let a = layer(&[1, 1, 1, 1, 1, 1], &[0, 0, 0, 1, 1, 1], &[1.0, 1.0]);
    let b = GroupedLayer::from_trits(
        2,
        3,
        &[1, 1, 1, 1, 1, 1],
        &[0, 0, 0, 1, 1, 1],
        &[1.0, 1.0],
        &[0.0, 0.0],
    )
    .expect("b");
    assert!(a.row_proportional(&b).is_some(), "同层自身应成比例(1.0)");

    // 用负 scale 构造的层：有效权重符号被翻，不应与正版本成"正比例"
    let negative = GroupedLayer::from_trits(
        2,
        3,
        &[1, 1, 1, 1, 1, 1],
        &[0, 0, 0, 1, 1, 1],
        &[-1.0, -1.0],
        &[0.0, 0.0],
    )
    .expect("negative");
    assert!(
        a.row_proportional(&negative).is_none(),
        "负比例应被拒绝（符号会翻转）"
    );
}

#[test]
fn shape_mismatch_returns_none() {
    let a = layer(&[1, 1, 1, 1, 1, 1], &[0, 0, 0, 1, 1, 1], &[1.0, 1.0]);
    let b = GroupedLayer::from_trits(
        1,
        6,
        &[1, 1, 1, 1, 1, 1],
        &[0, 0, 0, 0, 0, 0],
        &[1.0],
        &[0.0],
    )
    .expect("b");
    assert!(a.row_proportional(&b).is_none(), "形状不同应返回 None");
}
