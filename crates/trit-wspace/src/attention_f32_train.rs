//! f32 位置编码的反向传播训练：Q/K/V 固定，只学位置偏置。
//!
//! 损失是目标 key 的交叉熵：`L = -ln softmax(score)[target]`；
//! 对位置偏置的解析梯度就是 `softmax[i] - [i == target]`，用 SGD 更新。
//! 这是真正的反向传播，不是有限差分敏感度。

use crate::attention_f32::F32Attention;
use crate::error::Result;

/// 训练选项。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrainOptions {
    /// 轮数。
    pub rounds: usize,
    /// 学习率。
    pub lr: f32,
    /// 目标 key。
    pub target: usize,
}

/// 训练报告。
#[derive(Clone, Debug, PartialEq)]
pub struct TrainReport {
    /// 学到的位置偏置（每个 key 一个）。
    pub positions: Vec<f32>,
    /// 每轮损失。
    pub losses: Vec<f32>,
    /// 每轮梯度范数。
    pub grad_norms: Vec<f32>,
}

/// 只训练位置偏置。
pub fn train_positions(
    head: &F32Attention,
    query: &[f32],
    keys: &[Vec<f32>],
    values: &[Vec<f32>],
    options: &TrainOptions,
) -> Result<TrainReport> {
    let mut positions = vec![0.0f32; keys.len()];
    let mut losses = Vec::with_capacity(options.rounds);
    let mut grad_norms = Vec::with_capacity(options.rounds);
    for _ in 0..options.rounds {
        let trace = head.forward(query, keys, values)?;
        let mut scores = trace.scores.clone();
        for (score, position) in scores.iter_mut().zip(&positions) {
            *score += position;
        }
        let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = scores.iter().map(|score| (score - max).exp()).collect();
        let total: f32 = exps.iter().sum();
        let weights: Vec<f32> = exps.iter().map(|value| value / total).collect();
        let target = options.target.min(weights.len().saturating_sub(1));
        losses.push(-weights[target].max(1e-12).ln());
        let mut grad = Vec::with_capacity(weights.len());
        let mut norm = 0.0f32;
        for (index, weight) in weights.iter().enumerate() {
            let value = weight - if index == target { 1.0 } else { 0.0 };
            norm += value * value;
            grad.push(value);
        }
        grad_norms.push(norm.sqrt());
        for (position, value) in positions.iter_mut().zip(&grad) {
            *position -= options.lr * value;
        }
    }
    Ok(TrainReport {
        positions,
        losses,
        grad_norms,
    })
}
