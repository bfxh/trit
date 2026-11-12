//! 合并保留 f32 偏置：两网投票、对齐、任务向量（等权/加权/DARE）与缩放。

use trit_wspace::{
    Layer, Net, TaskVector, merge_aligned, merge_tasks, merge_tasks_weighted,
    merge_tasks_weighted_dare, merge_vote, scale_task,
};

fn net_with(hidden: Option<Vec<f32>>, output: Option<Vec<f32>>) -> Net {
    let mut first = Layer::new(2, 2, &[1, -1, -1, 1], &[1, -1]).unwrap();
    if let Some(values) = hidden {
        first = first.with_bias_f32(values).unwrap();
    }
    let mut last = Layer::new(1, 2, &[1, 1], &[-1]).unwrap();
    if let Some(values) = output {
        last = last.with_bias_f32(values).unwrap();
    }
    Net::new(vec![first, last]).unwrap()
}

fn base_net() -> Net {
    net_with(None, None)
}

fn relabel(layer: &Layer, values: Vec<f32>) -> Layer {
    let trits: Vec<i8> = layer.trits().iter().map(|trit| trit.to_i8()).collect();
    let bias: Vec<i8> = layer.bias().iter().map(|trit| trit.to_i8()).collect();
    Layer::with_scales(
        layer.rows(),
        layer.cols(),
        &trits,
        &bias,
        layer.scales().to_vec(),
    )
    .unwrap()
    .with_bias_f32(values)
    .unwrap()
}

#[test]
fn vote_averages_both_sides_and_leaves_behavior_alone() {
    let left = net_with(Some(vec![0.5, -0.25]), Some(vec![2.0]));
    let right = net_with(Some(vec![0.25, 0.75]), Some(vec![4.0]));
    let merged = merge_vote(&left, &right).unwrap();
    assert_eq!(merged.layers()[0].bias_f32(), &[0.375, 0.25]);
    assert_eq!(merged.layers()[1].bias_f32(), &[3.0]);
    // f32 偏置不参与行为判据。
    assert_eq!(merged.state_hash(), left.state_hash());
}

#[test]
fn vote_drops_f32_bias_when_either_side_missing() {
    let left = net_with(Some(vec![0.5, -0.25]), Some(vec![2.0]));
    let right = net_with(None, Some(vec![4.0]));
    let merged = merge_vote(&left, &right).unwrap();
    assert!(merged.layers()[0].bias_f32().is_empty());
    assert_eq!(merged.layers()[1].bias_f32(), &[3.0]);
}

#[test]
fn aligned_merge_moves_f32_bias_with_its_row() {
    let reference = net_with(Some(vec![0.5, -0.25]), Some(vec![2.0]));
    // 交换隐藏层两行（第二层列跟着换），再给行贴不同 f32 偏置。
    let permuted = reference.permute_rows(0, &[1, 0]).unwrap();
    // f32 偏置按置换后的行序重贴：行 0 对应原行 1、行 1 对应原行 0。
    let swapped = relabel(&permuted.layers()[0], vec![0.7, -0.05]);
    let permuted = Net::new(vec![swapped, permuted.layers()[1].clone()]).unwrap();
    let merged = merge_aligned(&reference, &permuted).unwrap();
    // 对齐后逐行平均：行 0 (0.5-0.05)/2、行 1 (-0.25+0.7)/2。
    assert_eq!(merged.layers()[0].bias_f32(), &[0.225, 0.225]);
}

#[test]
fn task_vectors_round_trip_f32_bias_both_directions() {
    let base = base_net();
    let tuned = net_with(Some(vec![0.5, -0.25]), Some(vec![2.0]));
    let vector = TaskVector::between(&base, &tuned).unwrap();
    let applied = vector.apply(&base).unwrap();
    assert_eq!(applied.layers()[0].bias_f32(), &[0.5, -0.25]);
    assert_eq!(applied.layers()[1].bias_f32(), &[2.0]);

    // 反向：基准有幅度、目标没有 → 应用后清空。
    let cleared = TaskVector::between(&tuned, &base)
        .unwrap()
        .apply(&tuned)
        .unwrap();
    assert!(cleared.layers()[0].bias_f32().is_empty());
    assert!(cleared.layers()[1].bias_f32().is_empty());
    assert_eq!(cleared.state_hash(), base.state_hash());
}

#[test]
fn merge_tasks_averages_f32_bias() {
    let base = base_net();
    let a = net_with(Some(vec![0.5, -0.25]), Some(vec![2.0]));
    let b = net_with(Some(vec![0.25, 0.75]), Some(vec![4.0]));
    let tasks = vec![
        TaskVector::between(&base, &a).unwrap(),
        TaskVector::between(&base, &b).unwrap(),
    ];
    let merged = merge_tasks(&base, &tasks).unwrap();
    assert_eq!(merged.layers()[0].bias_f32(), &[0.375, 0.25]);
    assert_eq!(merged.layers()[1].bias_f32(), &[3.0]);
}

#[test]
fn weighted_merge_uses_weights_and_ignores_zero() {
    let base = base_net();
    let a = net_with(Some(vec![0.0, -0.5]), Some(vec![0.0]));
    let b = net_with(Some(vec![4.0, 1.5]), Some(vec![8.0]));
    let tasks = vec![
        TaskVector::between(&base, &a).unwrap(),
        TaskVector::between(&base, &b).unwrap(),
    ];
    let merged = merge_tasks_weighted(&base, &tasks, &[1.0, 3.0]).unwrap();
    assert_eq!(merged.layers()[0].bias_f32(), &[3.0, 1.0]);
    assert_eq!(merged.layers()[1].bias_f32(), &[6.0]);

    // 权重 0 的任务完全不参与：结果就是另一个任务的幅度。
    let only_b = merge_tasks_weighted(&base, &tasks, &[0.0, 1.0]).unwrap();
    assert_eq!(only_b.layers()[0].bias_f32(), &[4.0, 1.5]);
    assert_eq!(only_b.layers()[1].bias_f32(), &[8.0]);
}

#[test]
fn a_task_without_f32_bias_clears_the_layer() {
    let base = net_with(Some(vec![0.25, 0.25]), Some(vec![1.0]));
    let with = net_with(Some(vec![0.5, -0.25]), Some(vec![2.0]));
    let without = net_with(None, Some(vec![3.0]));
    let tasks = vec![
        TaskVector::between(&base, &with).unwrap(),
        TaskVector::between(&base, &without).unwrap(),
    ];
    let merged = merge_tasks(&base, &tasks).unwrap();
    assert!(merged.layers()[0].bias_f32().is_empty());
    assert_eq!(merged.layers()[1].bias_f32(), &[2.5]);
}

#[test]
fn dare_keeps_the_merged_f32_bias() {
    let base = base_net();
    let a = net_with(Some(vec![0.5, -0.25]), Some(vec![2.0]));
    let b = net_with(Some(vec![0.25, 0.75]), Some(vec![4.0]));
    let tasks = vec![
        TaskVector::between(&base, &a).unwrap(),
        TaskVector::between(&base, &b).unwrap(),
    ];
    let merged = merge_tasks_weighted_dare(&base, &tasks, &[1.0, 1.0], 0.5, 7).unwrap();
    assert_eq!(merged.layers()[0].bias_f32(), &[0.375, 0.25]);
    assert_eq!(merged.layers()[1].bias_f32(), &[3.0]);
}

#[test]
fn scaling_keeps_f32_bias_only_for_positive_lambda() {
    let base = base_net();
    let tuned = net_with(Some(vec![0.5, -0.25]), Some(vec![2.0]));
    let vector = TaskVector::between(&base, &tuned).unwrap();
    let kept = scale_task(&vector, 2.0).unwrap();
    assert_eq!(kept.bias_f32().len(), 3);
    let dropped = scale_task(&vector, -1.0).unwrap();
    assert!(dropped.bias_f32().is_empty());
    let zero = scale_task(&vector, 0.0).unwrap();
    assert!(zero.bias_f32().is_empty());
}
