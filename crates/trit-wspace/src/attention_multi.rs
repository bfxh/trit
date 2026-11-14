//! 多头三值注意力 + 位置偏置。
//!
//! 每个头复用 [`AttentionHead`] 的 Q/K/V 三值前向；位置偏置直接加到整数 score 上，
//! 再重算 softmax 与加权输出。偏置是整数，所以「位置」本身也参与三值判决。

use crate::attention::{AttentionHead, AttentionTrace};
use crate::error::{Error, Result};
use crate::net::Layer;
use crate::trit::Trit;

/// 多头注意力。
#[derive(Clone, Debug, PartialEq)]
pub struct MultiHeadAttention {
    /// 各头。
    pub heads: Vec<AttentionHead>,
}

impl MultiHeadAttention {
    /// 至少一个头。
    pub fn new(heads: Vec<AttentionHead>) -> Result<Self> {
        if heads.is_empty() {
            return Err(Error::Shape {
                what: "heads",
                expected: 1,
                got: 0,
            });
        }
        Ok(Self { heads })
    }

    /// 对每个头跑一次；`position_bias` 为空时等价于单头原样输出。
    pub fn forward(
        &self,
        query: &[Trit],
        keys: &[Vec<Trit>],
        values: &[Vec<Trit>],
        position_bias: &[i32],
    ) -> Result<Vec<AttentionTrace>> {
        self.heads
            .iter()
            .map(|head| {
                let mut trace = head.forward(query, keys, values)?;
                if position_bias.is_empty() {
                    return Ok(trace);
                }
                for (score, bias) in trace.scores.iter_mut().zip(position_bias.iter().copied()) {
                    *score += bias;
                }
                let max = trace.scores.iter().copied().max().unwrap_or(0);
                let exps: Vec<f32> = trace
                    .scores
                    .iter()
                    .map(|score| ((score - max) as f32).exp())
                    .collect();
                let total: f32 = exps.iter().sum();
                trace.weights = if total == 0.0 {
                    vec![1.0 / keys.len() as f32; keys.len()]
                } else {
                    exps.iter().map(|value| value / total).collect()
                };
                trace.dominant = trace
                    .weights
                    .iter()
                    .enumerate()
                    .max_by(|left, right| {
                        left.1
                            .partial_cmp(right.1)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map_or(0, |(index, _)| index);
                trace.output = weighted_output(&head.value, &trace.weights, values);
                Ok(trace)
            })
            .collect()
    }
}

fn weighted_output(value: &Layer, weights: &[f32], values: &[Vec<Trit>]) -> Vec<f32> {
    let mut output = vec![0.0f32; value.rows()];
    for (weight, input) in weights.iter().zip(values) {
        let vector = layer_forward(value, input);
        for (slot, trit) in output.iter_mut().zip(&vector) {
            *slot += weight * f32::from(trit.to_i8());
        }
    }
    output
}

fn layer_forward(layer: &Layer, input: &[Trit]) -> Vec<Trit> {
    let mut out = Vec::with_capacity(layer.rows());
    for row in 0..layer.rows() {
        let mut acc = i32::from(layer.bias_trit(row).unwrap_or(Trit::Zero).to_i8());
        for col in 0..layer.cols() {
            let weight = layer.trit(row, col).unwrap_or(Trit::Zero).to_i8();
            let value = input.get(col).copied().unwrap_or(Trit::Zero).to_i8();
            acc += i32::from(weight) * i32::from(value);
        }
        out.push(Trit::sign_of(acc));
    }
    out
}
