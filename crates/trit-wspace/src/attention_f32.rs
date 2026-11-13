//! f32 注意力：Q/K/V 都是 f32、softmax f32——三值 attention 的对照。
//!
//! 结构与 [`crate::attention::AttentionHead`] 相同，只是不做三值化；
//! 用来回答「三值 attention 的 one-hot 是不是量化造成的」。

use crate::error::{Error, Result};

/// 一个 f32 注意力头。
#[derive(Clone, Debug, PartialEq)]
pub struct F32Attention {
    /// Query 权重（`rows × cols`）。
    pub query: Vec<f32>,
    /// Key 权重。
    pub key: Vec<f32>,
    /// Value 权重。
    pub value: Vec<f32>,
    /// 输出维。
    pub rows: usize,
    /// 输入维。
    pub cols: usize,
}

/// 一次 f32 注意力的 trace。
#[derive(Clone, Debug, PartialEq)]
pub struct F32Trace {
    /// 每个 key 的分数。
    pub scores: Vec<f32>,
    /// softmax 权重。
    pub weights: Vec<f32>,
    /// 加权输出。
    pub output: Vec<f32>,
    /// 权重最大的 key。
    pub dominant: usize,
}

impl F32Attention {
    /// 三块权重的长度必须一致且等于 `rows × cols`。
    pub fn new(
        query: Vec<f32>,
        key: Vec<f32>,
        value: Vec<f32>,
        rows: usize,
        cols: usize,
    ) -> Result<Self> {
        let expected = rows * cols;
        for (what, weights) in [("query", &query), ("key", &key), ("value", &value)] {
            if weights.len() != expected {
                return Err(Error::Shape {
                    what,
                    expected,
                    got: weights.len(),
                });
            }
        }
        Ok(Self {
            query,
            key,
            value,
            rows,
            cols,
        })
    }

    /// 一次单头 f32 注意力。
    pub fn forward(
        &self,
        query: &[f32],
        keys: &[Vec<f32>],
        values: &[Vec<f32>],
    ) -> Result<F32Trace> {
        if keys.len() != values.len() || keys.is_empty() {
            return Err(Error::Shape {
                what: "keys/values",
                expected: keys.len().max(1),
                got: values.len(),
            });
        }
        let q = project(&self.query, query, self.rows, self.cols);
        let scores: Vec<f32> = keys
            .iter()
            .map(|key| {
                let k = project(&self.key, key, self.rows, self.cols);
                q.iter().zip(&k).map(|(a, b)| a * b).sum()
            })
            .collect();
        let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = scores.iter().map(|score| (score - max).exp()).collect();
        let total: f32 = exps.iter().sum();
        let weights: Vec<f32> = if total == 0.0 {
            vec![1.0 / keys.len() as f32; keys.len()]
        } else {
            exps.iter().map(|value| value / total).collect()
        };
        let mut output = vec![0.0f32; self.rows];
        for (weight, value) in weights.iter().zip(values) {
            let v = project(&self.value, value, self.rows, self.cols);
            for (slot, component) in output.iter_mut().zip(&v) {
                *slot += weight * component;
            }
        }
        let dominant = weights
            .iter()
            .enumerate()
            .max_by(|left, right| {
                left.1
                    .partial_cmp(right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0, |(index, _)| index);
        Ok(F32Trace {
            scores,
            weights,
            output,
            dominant,
        })
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
