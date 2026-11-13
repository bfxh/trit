//! f32 transformer 层的结构测试。

use trit_wspace::attention_f32::F32Attention;
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};

fn identity_attention() -> F32Attention {
    F32Attention::new(
        vec![1.0, 0.0, 0.0, 1.0],
        vec![1.0, 0.0, 0.0, 1.0],
        vec![1.0, 0.0, 0.0, 1.0],
        2,
        2,
    )
    .expect("attention")
}

#[test]
fn layernorm_normalizes() {
    let ln = LayerNorm::new(vec![1.0, 1.0], vec![0.0, 0.0]).expect("ln");
    let out = ln.forward(&[1.0, 3.0]);
    assert!((out[0] + 1.0).abs() < 1e-3);
    assert!((out[1] - 1.0).abs() < 1e-3);
}

#[test]
fn dense_applies_relu() {
    let dense = Dense::new(vec![1.0, 0.0, 0.0, 1.0], vec![0.0, 0.0], 2, 2).expect("dense");
    assert_eq!(dense.forward(&[1.0, -3.0], false), vec![1.0, -3.0]);
    assert_eq!(dense.forward(&[1.0, -3.0], true), vec![1.0, 0.0]);
}

#[test]
fn transformer_block_forward_shape() {
    let block = TransformerBlock::new(
        identity_attention(),
        LayerNorm::new(vec![1.0, 1.0], vec![0.0, 0.0]).expect("ln1"),
        Dense::new(vec![1.0, 0.0, 0.0, 1.0], vec![0.0, 0.0], 2, 2).expect("ff1"),
        Dense::new(vec![1.0, 0.0, 0.0, 1.0], vec![0.0, 0.0], 2, 2).expect("ff2"),
        LayerNorm::new(vec![1.0, 1.0], vec![0.0, 0.0]).expect("ln2"),
    )
    .expect("block");
    let tokens = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let out = block.forward(&tokens).expect("forward");
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|row| row.len() == 2));
    assert!(out.iter().flatten().all(|value| value.is_finite()));
}
