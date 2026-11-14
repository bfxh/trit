//! Adam 训练的测试：收敛速度、抗大步长、梯度确实回到块参数。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::classifier_head::ClassifierHead;
use trit_wspace::classifier_train::{AdamOptions, train_classify_adam_with};
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

fn samples() -> (Vec<Vec<f32>>, Vec<usize>, Vec<Vec<f32>>) {
    let samples = vec![
        vec![1.0, 0.0, 0.0],
        vec![-1.0, 0.2, 0.0],
        vec![0.8, -0.1, 0.3],
        vec![-0.7, 0.1, -0.2],
    ];
    let labels = vec![1, 0, 1, 0];
    let context = vec![vec![1.0, 0.0, 0.0], vec![-1.0, 0.0, 0.0]];
    (samples, labels, context)
}

fn accuracy_at(
    block: &TransformerBlock,
    head: &ClassifierHead,
    samples: &[Vec<f32>],
    labels: &[usize],
    context: &[Vec<f32>],
) -> f32 {
    let features = forward_with(block, samples, context).expect("features");
    head.accuracy(&features, labels).expect("accuracy")
}

#[test]
fn adam_learns_with_few_steps() {
    let mut block = block();
    let mut head = ClassifierHead::zeros(2, 3).expect("head");
    let (samples, labels, context) = samples();
    let report = train_classify_adam_with(
        &mut block,
        &mut head,
        &samples,
        &context,
        &labels,
        &AdamOptions {
            rounds: 80,
            lr_block: 1e-3,
            lr_head: 1e-2,
            ..AdamOptions::default()
        },
    )
    .expect("adam train");
    let accuracy = accuracy_at(&block, &head, &samples, &labels, &context);
    assert!(report.losses[79] < report.losses[0], "交叉熵应下降");
    assert!(accuracy >= 0.75, "Adam 应在 80 步内学到: {accuracy}");
    assert!(report.losses.iter().all(|loss| loss.is_finite()));
}

#[test]
fn adam_beats_sgd_at_same_budget() {
    // 同样步数、同样学习率：Adam 的逐参数缩放应当让训练集准确率更高。
    let (samples, labels, context) = samples();

    let mut sgd_block = block();
    let mut sgd_head = ClassifierHead::zeros(2, 3).expect("head");
    train_classify_with(
        &mut sgd_block,
        &mut sgd_head,
        &samples,
        &context,
        &labels,
        &ClassifyOptions {
            rounds: 40,
            lr_block: 1e-3,
            lr_head: 1e-2,
        },
    )
    .expect("sgd train");
    let sgd_accuracy = accuracy_at(&sgd_block, &sgd_head, &samples, &labels, &context);

    let mut adam_block = block();
    let mut adam_head = ClassifierHead::zeros(2, 3).expect("head");
    train_classify_adam_with(
        &mut adam_block,
        &mut adam_head,
        &samples,
        &context,
        &labels,
        &AdamOptions {
            rounds: 40,
            lr_block: 1e-3,
            lr_head: 1e-2,
            ..AdamOptions::default()
        },
    )
    .expect("adam train");
    let adam_accuracy = accuracy_at(&adam_block, &adam_head, &samples, &labels, &context);

    assert!(
        adam_accuracy >= sgd_accuracy,
        "同预算下 Adam 不应劣于 SGD: adam={adam_accuracy} sgd={sgd_accuracy}"
    );
    assert!(adam_accuracy >= 0.75, "Adam 40 步应学到: {adam_accuracy}");
}

#[test]
fn adam_updates_block_parameters() {
    let mut block = block();
    let mut head = ClassifierHead::zeros(2, 3).expect("head");
    let (samples, labels, context) = samples();
    let before = block.ff2.weight.clone();
    train_classify_adam_with(
        &mut block,
        &mut head,
        &samples,
        &context,
        &labels,
        &AdamOptions {
            rounds: 5,
            lr_block: 1e-2,
            lr_head: 1e-2,
            ..AdamOptions::default()
        },
    )
    .expect("adam train");
    assert!(
        block
            .ff2
            .weight
            .iter()
            .zip(&before)
            .any(|(now, old)| (now - old).abs() > 1e-9),
        "Adam 的梯度应改到块参数，而不只是分类头"
    );
    assert!(block.ff2.weight.iter().all(|value| value.is_finite()));
}
