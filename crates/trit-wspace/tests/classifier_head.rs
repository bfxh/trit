//! 分类头（闭式岭回归）与"样本/上下文分离"训练的测试。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::classifier_head::ClassifierHead;
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_cls::{ClassifyOptions, forward_with, train_classify_with};

fn block() -> TransformerBlock {
    let identity = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    TransformerBlock::new(
        F32Attention::new(identity.clone(), identity.clone(), identity.clone(), 3, 3)
            .expect("attention"),
        LayerNorm::new(vec![1.0, 1.0, 1.0], vec![0.0, 0.0, 0.0]).expect("ln1"),
        Dense::new(identity.clone(), vec![0.0, 0.0, 0.0], 3, 3).expect("ff1"),
        Dense::new(identity, vec![0.0, 0.0, 0.0], 3, 3).expect("ff2"),
        LayerNorm::new(vec![1.0, 1.0, 1.0], vec![0.0, 0.0, 0.0]).expect("ln2"),
    )
    .expect("block")
}

#[test]
fn ridge_recovers_separable_labels() {
    // 第一维符号决定类别：线性可分，闭式解应当完全拟合且泛化到同分布点。
    let features: Vec<Vec<f32>> = (0..40)
        .map(|index| {
            let sign = if index % 2 == 0 { 1.0 } else { -1.0 };
            vec![
                sign,
                (index as f32 * 0.13).sin(),
                (index as f32 * 0.07).cos(),
            ]
        })
        .collect();
    let labels: Vec<usize> = (0..40).map(|index| index % 2).collect();
    let head = ClassifierHead::fit_ridge(&features, &labels, 1e-3).expect("ridge");
    let accuracy = head.accuracy(&features, &labels).expect("accuracy");
    assert!(accuracy >= 0.95, "闭式解应拟合可分标注: {accuracy}");
}

#[test]
fn ridge_rejects_bad_shapes() {
    assert!(ClassifierHead::fit_ridge(&[], &[], 1.0).is_none());
    assert!(ClassifierHead::fit_ridge(&[vec![1.0, 2.0]], &[0, 1], 1.0).is_none());
}

#[test]
fn forward_with_uses_given_context() {
    let block = block();
    let samples = vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
    let context = vec![vec![0.0, 1.0, 0.0], vec![0.0, 0.0, 1.0]];
    let with_context = forward_with(&block, &samples, &context).expect("forward_with");
    let without = forward_with(&block, &samples, &samples).expect("forward_with");
    assert_eq!(with_context.len(), 2);
    assert!(
        with_context[0]
            .iter()
            .zip(&without[0])
            .any(|(left, right)| (left - right).abs() > 1e-6),
        "换上下文应当改变注意力输出"
    );
}

#[test]
fn separated_context_training_lifts_accuracy() {
    let mut block = block();
    let mut head = ClassifierHead::zeros(2, 3).expect("head");
    let samples = vec![
        vec![1.0, 0.0, 0.0],
        vec![-1.0, 0.2, 0.0],
        vec![0.8, -0.1, 0.3],
        vec![-0.7, 0.1, -0.2],
    ];
    let labels = vec![1, 0, 1, 0];
    let context = vec![vec![1.0, 0.0, 0.0], vec![-1.0, 0.0, 0.0]];
    let report = train_classify_with(
        &mut block,
        &mut head,
        &samples,
        &context,
        &labels,
        &ClassifyOptions {
            rounds: 300,
            lr_block: 1e-4,
            lr_head: 0.1,
        },
    )
    .expect("train");
    let features = forward_with(&block, &samples, &context).expect("features");
    let accuracy = head.accuracy(&features, &labels).expect("accuracy");
    assert!(report.losses[299] < report.losses[0], "交叉熵应下降");
    assert!(accuracy >= 0.75, "分离上下文下也应学到: {accuracy}");
}
