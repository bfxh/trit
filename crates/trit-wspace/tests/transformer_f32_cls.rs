//! 分类头 + 交叉熵训练的测试。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_cls::{ClassifierHead, ClassifyOptions, train_classify};

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

fn samples() -> (Vec<Vec<f32>>, Vec<usize>) {
    let tokens = vec![
        vec![1.0, 0.0, 0.0],
        vec![-1.0, 0.2, 0.0],
        vec![0.8, -0.1, 0.3],
        vec![-0.7, 0.1, -0.2],
    ];
    // 标签只由第一维符号决定：2 正 2 负。
    let labels = vec![1, 0, 1, 0];
    (tokens, labels)
}

fn accuracy(
    block: &TransformerBlock,
    head: &ClassifierHead,
    tokens: &[Vec<f32>],
    labels: &[usize],
) -> f32 {
    let outs = block.forward(tokens).expect("forward");
    let hits = outs
        .iter()
        .zip(labels)
        .filter(|(out, label)| head.predict(out).expect("predict") == **label)
        .count();
    hits as f32 / labels.len() as f32
}

#[test]
fn cross_entropy_lifts_accuracy_above_chance() {
    let mut block = block();
    let mut head = ClassifierHead::zeros(2, 3).expect("head");
    let (tokens, labels) = samples();
    let before = accuracy(&block, &head, &tokens, &labels);
    assert_eq!(before, 0.5, "零初始化头的准确率应等于多数类比例");

    let report = train_classify(
        &mut block,
        &mut head,
        &tokens,
        &labels,
        &ClassifyOptions {
            rounds: 300,
            lr_block: 1e-4,
            lr_head: 0.1,
        },
    )
    .expect("train");

    let after = accuracy(&block, &head, &tokens, &labels);
    assert_eq!(report.losses.len(), 300);
    assert_eq!(report.accuracies.len(), 300);
    assert!(
        report.losses[299] < report.losses[0],
        "交叉熵应下降: {} -> {}",
        report.losses[0],
        report.losses[299]
    );
    assert!(after > before, "准确率应超过机会线: {before} -> {after}");
    assert!(after >= 0.75, "300 轮后准确率应明显高于随机: {after}");
    assert!(report.losses.iter().all(|loss| loss.is_finite()));
}

#[test]
fn gradients_reach_block_parameters() {
    let mut block = block();
    let mut head = ClassifierHead::zeros(2, 3).expect("head");
    let (tokens, labels) = samples();
    let before = block.ff2.weight.clone();
    train_classify(
        &mut block,
        &mut head,
        &tokens,
        &labels,
        &ClassifyOptions {
            rounds: 5,
            lr_block: 1e-2,
            lr_head: 0.1,
        },
    )
    .expect("train");
    assert!(
        block
            .ff2
            .weight
            .iter()
            .zip(&before)
            .any(|(now, old)| (now - old).abs() > 1e-9),
        "交叉熵的梯度应回到 block 参数，而不只是分类头"
    );
    assert!(block.ff2.weight.iter().all(|value| value.is_finite()));
}

#[test]
fn label_out_of_range_is_rejected() {
    let mut block = block();
    let mut head = ClassifierHead::zeros(2, 3).expect("head");
    let (tokens, _) = samples();
    let labels = vec![0, 1, 2, 0];
    assert!(
        train_classify(
            &mut block,
            &mut head,
            &tokens,
            &labels,
            &ClassifyOptions {
                rounds: 1,
                lr_block: 1e-4,
                lr_head: 0.1,
            },
        )
        .is_err()
    );
}
