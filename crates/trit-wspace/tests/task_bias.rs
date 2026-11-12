//! merge_vote 的 f32 bias 保留。

use trit_wspace::{Layer, Net, merge_vote};

#[test]
fn merge_vote_averages_bias_f32() {
    let left = Net::new(vec![
        Layer::new(2, 2, &[1, -1, 0, 1], &[1, -1])
            .expect("l0")
            .with_bias_f32(vec![0.5, -0.25])
            .expect("bias"),
    ])
    .expect("left");
    let right = Net::new(vec![
        Layer::new(2, 2, &[1, -1, 0, 1], &[1, -1])
            .expect("l0")
            .with_bias_f32(vec![-0.5, 0.75])
            .expect("bias"),
    ])
    .expect("right");
    let merged = merge_vote(&left, &right).expect("merge");
    assert_eq!(merged.layers()[0].bias_f32(), &[0.0, 0.25]);
}

#[test]
fn merge_vote_drops_bias_f32_if_one_side_missing() {
    let left = Net::new(vec![
        Layer::new(2, 2, &[1, -1, 0, 1], &[1, -1])
            .expect("l0")
            .with_bias_f32(vec![0.5, -0.25])
            .expect("bias"),
    ])
    .expect("left");
    let right = Net::new(vec![
        Layer::new(2, 2, &[1, -1, 0, 1], &[1, -1]).expect("l0"),
    ])
    .expect("right");
    let merged = merge_vote(&left, &right).expect("merge");
    assert!(merged.layers()[0].bias_f32().is_empty());
}
