//! 多头联合训练：共享位置偏置 + 每头 Q/K 权重，交叉熵反向传播。
//!
//! 与 `attention_f32_train` 的区别：那边 Q/K 固定、只学位置；这里 Q/K 也参与训练，
//! 且所有头在同一个循环里更新共享的位置偏置——多头联合，而不是每头独立。

use crate::attention_f32::F32Attention;
use crate::error::Result;

/// 联合训练选项。
#[derive(Clone, Debug, PartialEq)]
pub struct JointOptions {
    /// 轮数。
    pub rounds: usize,
    /// 学习率。
    pub lr: f32,
    /// 每头的目标 dominant。
    pub targets: Vec<usize>,
}

/// 联合训练报告。
#[derive(Clone, Debug, PartialEq)]
pub struct JointReport {
    /// 共享位置偏置。
    pub positions: Vec<f32>,
    /// 每轮总损失（各头交叉熵之和）。
    pub losses: Vec<f32>,
    /// 每轮位置梯度范数。
    pub grad_norms: Vec<f32>,
}

/// 联合训练所有头。
pub fn train_joint(
    heads: &mut [F32Attention],
    query: &[f32],
    keys: &[Vec<f32>],
    options: &JointOptions,
) -> Result<JointReport> {
    let keys_len = keys.len();
    let mut positions = vec![0.0f32; keys_len];
    let mut losses = Vec::with_capacity(options.rounds);
    let mut grad_norms = Vec::with_capacity(options.rounds);
    for _ in 0..options.rounds {
        let mut dpos = vec![0.0f32; keys_len];
        let mut total_loss = 0.0f32;
        for (head_index, head) in heads.iter_mut().enumerate() {
            let target = options
                .targets
                .get(head_index)
                .copied()
                .unwrap_or(0)
                .min(keys_len.saturating_sub(1));
            let q = project(&head.query, query, head.rows, head.cols);
            let k: Vec<Vec<f32>> = keys
                .iter()
                .map(|key| project(&head.key, key, head.rows, head.cols))
                .collect();
            let mut scores: Vec<f32> = (0..keys_len)
                .map(|index| {
                    q.iter()
                        .zip(&k[index])
                        .map(|(left, right)| left * right)
                        .sum::<f32>()
                        + positions[index]
                })
                .collect();
            let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            for score in &mut scores {
                *score = (*score - max).exp();
            }
            let total: f32 = scores.iter().sum();
            let weights: Vec<f32> = scores.iter().map(|value| value / total).collect();
            total_loss += -weights[target].max(1e-12).ln();
            let dscore: Vec<f32> = (0..keys_len)
                .map(|index| weights[index] - if index == target { 1.0 } else { 0.0 })
                .collect();
            for (position, gradient) in dpos.iter_mut().zip(&dscore) {
                *position += gradient;
            }
            let mut dq = vec![0.0f32; head.rows];
            for (index, gradient) in dscore.iter().enumerate() {
                for (row, slot) in dq.iter_mut().enumerate() {
                    *slot += *gradient * k[index][row];
                }
            }
            for (row, gradient) in dq.iter().enumerate() {
                for (col, value) in query.iter().take(head.cols).enumerate() {
                    head.query[row * head.cols + col] -= options.lr * *gradient * *value;
                }
            }
            for (index, gradient) in dscore.iter().enumerate() {
                for (row, q_value) in q.iter().enumerate() {
                    let scalar = *gradient * *q_value;
                    for (col, value) in keys[index].iter().take(head.cols).enumerate() {
                        head.key[row * head.cols + col] -= options.lr * scalar * *value;
                    }
                }
            }
        }
        let norm = heads.len().max(1) as f32;
        for (position, gradient) in positions.iter_mut().zip(&dpos) {
            *position -= options.lr * gradient / norm;
        }
        losses.push(total_loss);
        grad_norms.push(dpos.iter().map(|value| value * value).sum::<f32>().sqrt());
    }
    Ok(JointReport {
        positions,
        losses,
        grad_norms,
    })
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
