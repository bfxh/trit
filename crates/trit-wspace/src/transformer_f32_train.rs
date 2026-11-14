//! transformer 块的局部反向传播：固定其他参数，只训练 FFN 第二层。
//!
//! 损失是输出与目标的 MSE；梯度经 `LN2 → 残差 → FFN2` 回传，
//! FFN1/注意力/LayerNorm 参数固定（forward-only 的其余部分）。

use crate::error::Result;
use crate::transformer_f32::{Dense, LayerNorm, TransformerBlock};

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

/// LayerNorm 对输入的解析梯度。
#[must_use]
pub fn layernorm_backward(layernorm: &LayerNorm, x: &[f32], grad_y: &[f32]) -> Vec<f32> {
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
    let sum_dhat: f32 = dhat.iter().sum();
    let sum_dhat_xhat: f32 = dhat.iter().zip(&xhat).map(|(d, xh)| d * xh).sum();
    (0..x.len())
        .map(|index| (dhat[index] - sum_dhat / count - xhat[index] * sum_dhat_xhat / count) / std)
        .collect()
}

/// Dense 反向：返回 `(grad_x, grad_weight, grad_bias)`。
#[must_use]
pub fn dense_backward(
    dense: &Dense,
    x: &[f32],
    grad_y: &[f32],
    relu: bool,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut grad_x = vec![0.0f32; dense.cols];
    let mut grad_weight = vec![0.0f32; dense.rows * dense.cols];
    let mut grad_bias = vec![0.0f32; dense.rows];
    for row in 0..dense.rows {
        let mut gradient = grad_y[row];
        if relu {
            let pre = dense.weight[row * dense.cols..(row + 1) * dense.cols]
                .iter()
                .zip(x)
                .map(|(weight, value)| weight * value)
                .sum::<f32>()
                + dense.bias[row];
            if pre <= 0.0 {
                gradient = 0.0;
            }
        }
        grad_bias[row] = gradient;
        for col in 0..dense.cols {
            grad_weight[row * dense.cols + col] = gradient * x[col];
            grad_x[col] += dense.weight[row * dense.cols + col] * gradient;
        }
    }
    (grad_x, grad_weight, grad_bias)
}

/// 训练 FFN 第二层，损失 = 输出与目标的 MSE。
pub fn train_ffn2(
    block: &mut TransformerBlock,
    tokens: &[Vec<f32>],
    targets: &[Vec<f32>],
    options: &TrainOptions,
) -> Result<TrainReport> {
    let mut losses = Vec::with_capacity(options.rounds);
    for _ in 0..options.rounds {
        let mut total = 0.0f32;
        let mut grad_weight = vec![0.0f32; block.ff2.weight.len()];
        let mut grad_bias = vec![0.0f32; block.ff2.bias.len()];
        for (token, target) in tokens.iter().zip(targets) {
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
            let count = x2.len() as f32;
            let diff: Vec<f32> = x2
                .iter()
                .zip(target)
                .map(|(out, want)| out - want)
                .collect();
            total += diff.iter().map(|value| value * value).sum::<f32>() / count;
            let grad_x2: Vec<f32> = diff.iter().map(|value| 2.0 * value / count).collect();
            let grad_y = layernorm_backward(&block.ln2, &x2_raw, &grad_x2);
            let (_, weight_grad, bias_grad) = dense_backward(&block.ff2, &hidden, &grad_y, false);
            for (slot, gradient) in grad_weight.iter_mut().zip(&weight_grad) {
                *slot += gradient;
            }
            for (slot, gradient) in grad_bias.iter_mut().zip(&bias_grad) {
                *slot += gradient;
            }
        }
        let count = tokens.len().max(1) as f32;
        for (weight, gradient) in block.ff2.weight.iter_mut().zip(&grad_weight) {
            *weight -= options.lr * gradient / count;
        }
        for (bias, gradient) in block.ff2.bias.iter_mut().zip(&grad_bias) {
            *bias -= options.lr * gradient / count;
        }
        losses.push(total / count);
    }
    Ok(TrainReport { losses })
}
