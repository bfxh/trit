//! 单 token 的 forward 与反向：MSE（`super::train_full`）与分类（`crate::transformer_f32_cls`）共用。
//!
//! 抽出来是为了让两条损失走**同一份**反向实现，而不是各复制一套训练循环。

use super::{attention_backward, layernorm_backward_full};
use crate::transformer_f32::TransformerBlock;
use crate::transformer_f32_train::dense_backward;
use crate::{error::Result, grads::Grads};

/// 单个 token 的 forward 中间量，供反向复用。
#[derive(Clone, Debug)]
pub(crate) struct TokenCache {
    x1_raw: Vec<f32>,
    x1: Vec<f32>,
    hidden: Vec<f32>,
    x2_raw: Vec<f32>,
    x2: Vec<f32>,
}

impl TokenCache {
    /// 块输出（LN2 之后）。
    pub(crate) fn output(&self) -> &[f32] {
        &self.x2
    }
}

/// 单个 token 的 forward；`tokens` 同时充当 attention 的 K/V 上下文。
pub(crate) fn forward_token(
    block: &TransformerBlock,
    token: &[f32],
    tokens: &[Vec<f32>],
) -> Result<TokenCache> {
    let attention = block.attention.forward(token, tokens, tokens)?.output;
    let x1_raw: Vec<f32> = token
        .iter()
        .zip(&attention)
        .map(|(left, right)| left + right)
        .collect();
    let x1 = block.ln1.forward(&x1_raw);
    let hidden = block.ff1.forward(&x1, true);
    let ffn = block.ff2.forward(&hidden, false);
    let x2_raw: Vec<f32> = x1
        .iter()
        .zip(&ffn)
        .map(|(left, right)| left + right)
        .collect();
    let x2 = block.ln2.forward(&x2_raw);
    Ok(TokenCache {
        x1_raw,
        x1,
        hidden,
        x2_raw,
        x2,
    })
}

/// 所有 token 的 forward。
pub(crate) fn forward_all(
    block: &TransformerBlock,
    tokens: &[Vec<f32>],
) -> Result<Vec<TokenCache>> {
    tokens
        .iter()
        .map(|token| forward_token(block, token, tokens))
        .collect()
}

/// 单个 token 的反向：把 `grad_x2` 回传到 attention 与全部参数。
pub(crate) fn backward_token(
    block: &TransformerBlock,
    token: &[f32],
    tokens: &[Vec<f32>],
    cache: &TokenCache,
    grad_x2: &[f32],
    grads: &mut Grads,
) {
    let (grad_x2_raw, gw_ln2, gb_ln2) = layernorm_backward_full(&block.ln2, &cache.x2_raw, grad_x2);
    let (grad_hidden, gw_ff2, gb_ff2) =
        dense_backward(&block.ff2, &cache.hidden, &grad_x2_raw, false);
    let (grad_x1_ff, gw_ff1, gb_ff1) = dense_backward(&block.ff1, &cache.x1, &grad_hidden, true);
    let grad_x1: Vec<f32> = grad_x2_raw
        .iter()
        .zip(&grad_x1_ff)
        .map(|(left, right)| left + right)
        .collect();
    let (grad_x1_raw, gw_ln1, gb_ln1) =
        layernorm_backward_full(&block.ln1, &cache.x1_raw, &grad_x1);
    let attention_grad = attention_backward(&block.attention, token, tokens, tokens, &grad_x1_raw);
    grads.add(
        &attention_grad,
        &gw_ln1,
        &gb_ln1,
        &gw_ln2,
        &gb_ln2,
        &gw_ff1,
        &gb_ff1,
        &gw_ff2,
        &gb_ff2,
    );
}
