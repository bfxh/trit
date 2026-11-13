//! transformer 块的全参数反向传播：注意力 Q/K/V、两层 LayerNorm、两层 FFN 都训练。
//!
//! 损失是输出与目标的 MSE；梯度从输出一路回传到注意力与所有参数。

use crate::attention_f32::F32Attention;
use crate::error::Result;
use crate::transformer_f32::{LayerNorm, TransformerBlock};

/// 训练选项。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrainOptions {
    /// 轮数。
    pub rounds: usize,
    /// 学习率。
    pub lr: f32,
}

/// 训练报告。
#[derive(Clone, Debug, PartialEq)]
pub struct TrainReport {
    /// 每轮平均 MSE。
    pub losses: Vec<f32>,
}

/// 一次 attention 的反向结果。
#[derive(Clone, Debug, PartialEq)]
pub struct AttentionGrad {
    /// 对 Q 权重的梯度。
    pub query: Vec<f32>,
    /// 对 K 权重的梯度。
    pub key: Vec<f32>,
    /// 对 V 权重的梯度。
    pub value: Vec<f32>,
}

/// LayerNorm 的反向：`(grad_x, grad_weight, grad_bias)`。
#[must_use]
pub fn layernorm_backward_full(
    layernorm: &LayerNorm,
    x: &[f32],
    grad_y: &[f32],
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let count = x.len() as f32;
    let mean = x.iter().sum::<f32>() / count;
    let variance = x
        .iter()
        .map(|value| (value - mean) * (value - mean))
        .sum::<f32>()
        / count;
    let std = (variance + layernorm.eps).sqrt();
    let xhat: Vec<f32> = x.iter().map(|value| (value - mean) / std).collect();
    let dhat: Vec<f32> = grad_y
        .iter()
        .zip(&layernorm.weight)
        .map(|(g, w)| g * w)
        .collect();
    let grad_weight: Vec<f32> = grad_y.iter().zip(&xhat).map(|(g, xh)| g * xh).collect();
    let grad_bias = grad_y.to_vec();
    let sum_dhat: f32 = dhat.iter().sum();
    let sum_dhat_xhat: f32 = dhat.iter().zip(&xhat).map(|(d, xh)| d * xh).sum();
    let grad_x = (0..x.len())
        .map(|index| (dhat[index] - sum_dhat / count - xhat[index] * sum_dhat_xhat / count) / std)
        .collect();
    (grad_x, grad_weight, grad_bias)
}

/// 单头 attention 的反向：返回 Q/K/V 权重的梯度。
#[must_use]
pub fn attention_backward(
    attention: &F32Attention,
    query: &[f32],
    keys: &[Vec<f32>],
    values: &[Vec<f32>],
    grad_out: &[f32],
) -> AttentionGrad {
    let dim = attention.rows;
    let q = project(&attention.query, query, dim, attention.cols);
    let k: Vec<Vec<f32>> = keys
        .iter()
        .map(|key| project(&attention.key, key, dim, attention.cols))
        .collect();
    let v: Vec<Vec<f32>> = values
        .iter()
        .map(|value| project(&attention.value, value, dim, attention.cols))
        .collect();
    let scores: Vec<f32> = (0..keys.len())
        .map(|index| q.iter().zip(&k[index]).map(|(a, b)| a * b).sum())
        .collect();
    let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = scores.iter().map(|score| (score - max).exp()).collect();
    let total: f32 = exps.iter().sum();
    let weights: Vec<f32> = exps.iter().map(|value| value / total).collect();
    let mut grad_value = vec![0.0f32; attention.value.len()];
    for (index, weight) in weights.iter().enumerate() {
        for row in 0..dim {
            let gradient = weight * grad_out[row];
            for col in 0..attention.cols {
                grad_value[row * attention.cols + col] += gradient * values[index][col];
            }
        }
    }
    let dw: Vec<f32> = (0..keys.len())
        .map(|index| {
            grad_out
                .iter()
                .zip(&v[index])
                .map(|(g, value)| g * value)
                .sum()
        })
        .collect();
    let weighted: f32 = weights
        .iter()
        .zip(&dw)
        .map(|(weight, value)| weight * value)
        .sum();
    let dscores: Vec<f32> = (0..keys.len())
        .map(|index| weights[index] * (dw[index] - weighted))
        .collect();
    let mut dq = vec![0.0f32; dim];
    for index in 0..keys.len() {
        for row in 0..dim {
            dq[row] += dscores[index] * k[index][row];
        }
    }
    let mut grad_query = vec![0.0f32; attention.query.len()];
    for row in 0..dim {
        for col in 0..attention.cols {
            grad_query[row * attention.cols + col] += dq[row] * query[col];
        }
    }
    let mut grad_key = vec![0.0f32; attention.key.len()];
    for index in 0..keys.len() {
        for row in 0..dim {
            let gradient = dscores[index] * q[row];
            for col in 0..attention.cols {
                grad_key[row * attention.cols + col] += gradient * keys[index][col];
            }
        }
    }
    AttentionGrad {
        query: grad_query,
        key: grad_key,
        value: grad_value,
    }
}

pub(crate) mod step;

use step::{backward_token, forward_all};

/// 全参数训练：一轮 forward/backward，更新所有参数。
pub fn train_full(
    block: &mut TransformerBlock,
    tokens: &[Vec<f32>],
    targets: &[Vec<f32>],
    options: &TrainOptions,
) -> Result<TrainReport> {
    let mut losses = Vec::with_capacity(options.rounds);
    for _ in 0..options.rounds {
        let caches = forward_all(block, tokens)?;
        let mut total = 0.0f32;
        let mut grads = Grads::default();
        for ((cache, target), token) in caches.iter().zip(targets).zip(tokens) {
            let output = cache.output();
            let count = output.len() as f32;
            let diff: Vec<f32> = output
                .iter()
                .zip(target)
                .map(|(out, want)| out - want)
                .collect();
            total += diff.iter().map(|value| value * value).sum::<f32>() / count;
            let grad_x2: Vec<f32> = diff.iter().map(|value| 2.0 * value / count).collect();
            backward_token(block, token, tokens, cache, &grad_x2, &mut grads);
        }
        grads.apply(block, options.lr, tokens.len().max(1) as f32);
        losses.push(total / tokens.len().max(1) as f32);
    }
    Ok(TrainReport { losses })
}

#[derive(Default)]
pub(crate) struct Grads {
    query: Vec<f32>,
    key: Vec<f32>,
    value: Vec<f32>,
    ln1_weight: Vec<f32>,
    ln1_bias: Vec<f32>,
    ln2_weight: Vec<f32>,
    ln2_bias: Vec<f32>,
    ff1_weight: Vec<f32>,
    ff1_bias: Vec<f32>,
    ff2_weight: Vec<f32>,
    ff2_bias: Vec<f32>,
}

#[allow(clippy::too_many_arguments)]
impl Grads {
    pub(crate) fn add(
        &mut self,
        attention: &AttentionGrad,
        gw_ln1: &[f32],
        gb_ln1: &[f32],
        gw_ln2: &[f32],
        gb_ln2: &[f32],
        gw_ff1: &[f32],
        gb_ff1: &[f32],
        gw_ff2: &[f32],
        gb_ff2: &[f32],
    ) {
        accumulate(&mut self.query, &attention.query);
        accumulate(&mut self.key, &attention.key);
        accumulate(&mut self.value, &attention.value);
        accumulate(&mut self.ln1_weight, gw_ln1);
        accumulate(&mut self.ln1_bias, gb_ln1);
        accumulate(&mut self.ln2_weight, gw_ln2);
        accumulate(&mut self.ln2_bias, gb_ln2);
        accumulate(&mut self.ff1_weight, gw_ff1);
        accumulate(&mut self.ff1_bias, gb_ff1);
        accumulate(&mut self.ff2_weight, gw_ff2);
        accumulate(&mut self.ff2_bias, gb_ff2);
    }

    pub(crate) fn apply(&self, block: &mut TransformerBlock, lr: f32, count: f32) {
        apply(&mut block.attention.query, &self.query, lr, count);
        apply(&mut block.attention.key, &self.key, lr, count);
        apply(&mut block.attention.value, &self.value, lr, count);
        apply(&mut block.ln1.weight, &self.ln1_weight, lr, count);
        apply(&mut block.ln1.bias, &self.ln1_bias, lr, count);
        apply(&mut block.ln2.weight, &self.ln2_weight, lr, count);
        apply(&mut block.ln2.bias, &self.ln2_bias, lr, count);
        apply(&mut block.ff1.weight, &self.ff1_weight, lr, count);
        apply(&mut block.ff1.bias, &self.ff1_bias, lr, count);
        apply(&mut block.ff2.weight, &self.ff2_weight, lr, count);
        apply(&mut block.ff2.bias, &self.ff2_bias, lr, count);
    }
}

fn accumulate(target: &mut Vec<f32>, source: &[f32]) {
    if target.len() != source.len() {
        *target = vec![0.0; source.len()];
    }
    for (slot, value) in target.iter_mut().zip(source) {
        *slot += value;
    }
}

fn apply(target: &mut [f32], gradient: &[f32], lr: f32, count: f32) {
    for (slot, value) in target.iter_mut().zip(gradient) {
        *slot -= lr * value / count;
    }
}

fn project(weights: &[f32], input: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    (0..rows)
        .map(|row| {
            let line = &weights[row * cols..(row + 1) * cols];
            line.iter()
                .zip(input)
                .map(|(weight, value)| weight * value)
                .sum()
        })
        .collect()
}
