//! transformer 全参数反向传播的测试。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_full::{TrainOptions, train_full};

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
fn full_training_reduces_loss() {
    let mut block = block();
    let tokens = vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
    let targets = vec![vec![0.0, 1.0, 0.0], vec![1.0, 0.0, 0.0]];
    let report = train_full(
        &mut block,
        &tokens,
        &targets,
        &TrainOptions {
            rounds: 100,
            lr: 0.01,
        },
    )
    .expect("train");
    assert_eq!(report.losses.len(), 100);
    assert!(report.losses[99] < report.losses[0]);
    assert!(report.losses.iter().all(|loss| loss.is_finite()));
    assert!(
        block
            .ff2
            .weight
            .iter()
            .chain(&block.attention.query)
            .all(|value| value.is_finite())
    );
}
