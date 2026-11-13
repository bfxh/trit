//! transformer 局部反向传播的测试。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_train::{
    TrainOptions, dense_backward, layernorm_backward, train_ffn2,
};

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
fn layernorm_backward_has_zero_mean() {
    let ln = LayerNorm::new(vec![1.0, 1.0], vec![0.0, 0.0]).expect("ln");
    let grad = layernorm_backward(&ln, &[1.0, 3.0], &[1.0, 1.0]);
    assert!((grad[0] + grad[1]).abs() < 1e-5);
}

#[test]
fn dense_backward_shapes() {
    let dense = Dense::new(vec![1.0, 2.0, 3.0, 4.0], vec![0.0, 0.0], 2, 2).expect("dense");
    let (grad_x, grad_w, grad_b) = dense_backward(&dense, &[1.0, 1.0], &[1.0, 1.0], false);
    assert_eq!(grad_x.len(), 2);
    assert_eq!(grad_w.len(), 4);
    assert_eq!(grad_b.len(), 2);
}

#[test]
fn training_ffn2_reduces_loss() {
    let mut block = block();
    let tokens = vec![vec![1.0, 0.0, 0.0]];
    let targets = vec![vec![0.0, 1.0, 0.0]];
    let report = train_ffn2(
        &mut block,
        &tokens,
        &targets,
        &TrainOptions {
            rounds: 100,
            lr: 0.1,
        },
    )
    .expect("train");
    assert_eq!(report.losses.len(), 100);
    assert!(report.losses[99] < report.losses[0]);
}
