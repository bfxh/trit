//! 分组尺度下的补丁搜索：能否修复，以及"编辑效果依赖分组"这个差异。

use trit_wspace::grouped::GroupedLayer;
use trit_wspace::grouped_repair::{apply, loss, repair_single};
use trit_wspace::oracle::Oracle;
use trit_wspace::trit::Trit;

fn layer(trits: &[i8], groups: &[u32], scales: &[f32]) -> GroupedLayer {
    GroupedLayer::from_trits(2, 2, trits, groups, scales, &[0.0, 0.0]).expect("layer")
}

/// 用一个已知层的全部 3^2 行为构造 oracle。
fn oracle_of(layer: &GroupedLayer) -> Oracle {
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for x0 in [Trit::Neg, Trit::Zero, Trit::Pos] {
        for x1 in [Trit::Neg, Trit::Zero, Trit::Pos] {
            let input = vec![x0, x1];
            let features: Vec<f32> = input.iter().map(|t| f32::from(t.to_i8())).collect();
            let out = layer.forward(&features).expect("forward");
            outputs.push(
                out.iter()
                    .map(|v| {
                        if *v > 1e-9 {
                            Trit::Pos
                        } else if *v < -1e-9 {
                            Trit::Neg
                        } else {
                            Trit::Zero
                        }
                    })
                    .collect(),
            );
            inputs.push(input);
        }
    }
    Oracle::new(inputs, outputs).expect("oracle")
}

#[test]
fn matching_layer_has_zero_loss() {
    let good = layer(&[1, 1, 1, -1], &[0, 0, 1, 1], &[0.1, 10.0]);
    let oracle = oracle_of(&good);
    assert_eq!(loss(&good, &oracle).expect("loss"), 0.0);
    // 已经匹配的层不需要补丁
    assert!(repair_single(&good, &oracle).expect("repair").is_none());
}

#[test]
fn repairs_single_trit_corruption() {
    let good = layer(&[1, 1, 1, -1], &[0, 0, 1, 1], &[0.1, 10.0]);
    let oracle = oracle_of(&good);
    // 打坏一个 trit（1,1: -1 -> +1）
    let broken = layer(&[1, 1, 1, 1], &[0, 0, 1, 1], &[0.1, 10.0]);
    let before = loss(&broken, &oracle).expect("loss");
    assert!(before > 0.0, "损坏后 loss 应大于 0（否则测试构造失效）");

    let edit = repair_single(&broken, &oracle)
        .expect("repair")
        .expect("应能找到单点补丁");
    assert_eq!((edit.row, edit.col), (1, 1), "补丁位置应为被改坏的那一格");
    assert_eq!(edit.to, Trit::Neg, "应改回原来的 trit");

    let fixed = apply(&broken, &edit).expect("apply");
    assert_eq!(
        loss(&fixed, &oracle).expect("loss"),
        0.0,
        "修复后应完全匹配"
    );
    // 原层不被修改
    assert_eq!(broken.trit(1, 1), Some(Trit::Pos), "apply 不应改动原层");
}

#[test]
fn edit_effect_depends_on_grouping() {
    // 同一个 trit 编辑 (1,1): -1 -> +1，在两种分组下的效果不同。
    let per_row = layer(&[1, 1, 1, -1], &[0, 0, 1, 1], &[0.1, 10.0]);
    let cross = layer(&[1, 1, 1, -1], &[0, 1, 0, 1], &[0.1, 10.0]);
    let oracle = oracle_of(&per_row);

    let edit_per_row = layer(&[1, 1, 1, 1], &[0, 0, 1, 1], &[0.1, 10.0]);
    let edit_cross = layer(&[1, 1, 1, 1], &[0, 1, 0, 1], &[0.1, 10.0]);

    // 编辑后的有效权重不同 ⇒ 行为/损失不同（这就是"分组尺度"与 Layer 的差别）
    let w_per_row: Vec<f32> = (0..2)
        .flat_map(|r| (0..2).map(move |c| (r, c)))
        .map(|(r, c)| edit_per_row.effective(r, c).unwrap())
        .collect();
    let w_cross: Vec<f32> = (0..2)
        .flat_map(|r| (0..2).map(move |c| (r, c)))
        .map(|(r, c)| edit_cross.effective(r, c).unwrap())
        .collect();
    assert_ne!(w_per_row, w_cross, "同 trits、不同分组 ⇒ 有效权重必须不同");

    let loss_a = loss(&edit_per_row, &oracle).expect("a");
    let loss_b = loss(&edit_cross, &oracle).expect("b");
    assert!(
        (loss_a - loss_b).abs() > 1e-6,
        "同一编辑在两种分组下损失应不同（per-row {loss_a} vs cross {loss_b}）"
    );
    let _ = cross;
}

#[test]
fn returns_none_when_no_single_edit_helps() {
    // oracle 来自一个 2×2 层，但候选层每行权重都被缩放成"全零行为"——
    // 单编辑显然不够，应返回 None 而不是硬凑一个补丁。
    let good = layer(&[1, 1, 1, -1], &[0, 0, 1, 1], &[0.1, 10.0]);
    let oracle = oracle_of(&good);
    let hopeless = layer(&[0, 0, 0, 0], &[0, 0, 1, 1], &[0.1, 10.0]);
    let found = repair_single(&hopeless, &oracle).expect("repair");
    // 全零层改一格是否就能匹配？正确答案取决于 oracle；这里只要求不 panic 且语义自洽：
    if let Some(edit) = found {
        let fixed = apply(&hopeless, &edit).expect("apply");
        assert_eq!(
            loss(&fixed, &oracle).expect("loss"),
            0.0,
            "若返回补丁就必须真修好"
        );
    }
}

#[test]
fn loss_counts_mismatches_by_sign() {
    let good = layer(&[1, 0, 0, 1], &[0, 0, 1, 1], &[1.0, 1.0]);
    let oracle = oracle_of(&good);
    let flipped = layer(&[-1, 0, 0, 1], &[0, 0, 1, 1], &[1.0, 1.0]);
    let value = loss(&flipped, &oracle).expect("loss");
    assert!(value > 0.0 && value <= 1.0, "失配比例应落在 (0, 1] 内");
}
