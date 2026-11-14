//! 每头独立的位置编码训练：Q/K 固定，每头一份位置表。
//!
//! 与 `attention_f32_joint` 的共享位置不同，这里每头独立，避免多头目标冲突。

use crate::attention_f32::F32Attention;
use crate::attention_f32_train::{TrainOptions, train_positions};
use crate::error::Result;

/// 独立训练选项。
#[derive(Clone, Debug, PartialEq)]
pub struct IndependentOptions {
    /// 轮数。
    pub rounds: usize,
    /// 学习率。
    pub lr: f32,
    /// 每头的目标 dominant。
    pub targets: Vec<usize>,
}

/// 单头结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadResult {
    /// 头号。
    pub head: usize,
    /// 训练前 loss。
    pub loss_before: f32,
    /// 训练后 loss。
    pub loss_after: f32,
    /// 训练后的 dominant。
    pub dominant: usize,
    /// 是否匹配目标。
    pub matched: bool,
}

/// 独立训练报告。
#[derive(Clone, Debug, PartialEq)]
pub struct IndependentReport {
    /// 每头结果。
    pub heads: Vec<HeadResult>,
    /// 匹配数。
    pub matched: usize,
    /// 总头数。
    pub total: usize,
}

/// 每头独立训练位置偏置。
pub fn train_independent(
    heads: &[F32Attention],
    query: &[f32],
    keys: &[Vec<f32>],
    values: &[Vec<f32>],
    options: &IndependentOptions,
) -> Result<IndependentReport> {
    let mut results = Vec::with_capacity(heads.len());
    let mut matched = 0usize;
    for (index, head) in heads.iter().enumerate() {
        let target = options
            .targets
            .get(index)
            .copied()
            .unwrap_or(0)
            .min(keys.len().saturating_sub(1));
        let report = train_positions(
            head,
            query,
            keys,
            values,
            &TrainOptions {
                rounds: options.rounds,
                lr: options.lr,
                target,
            },
        )?;
        let trace = head.forward(query, keys, values)?;
        let mut best = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for (position, (score, bias)) in trace.scores.iter().zip(&report.positions).enumerate() {
            let value = score + bias;
            if value > best_score {
                best_score = value;
                best = position;
            }
        }
        let hit = best == target;
        if hit {
            matched += 1;
        }
        results.push(HeadResult {
            head: index,
            loss_before: report.losses.first().copied().unwrap_or(0.0),
            loss_after: report.losses.last().copied().unwrap_or(0.0),
            dominant: best,
            matched: hit,
        });
    }
    Ok(IndependentReport {
        heads: results,
        matched,
        total: heads.len(),
    })
}
