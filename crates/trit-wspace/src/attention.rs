//! 三值单头注意力：Q/K/V 权重三值，分数整数点积，softmax 用 f32。
//!
//! 这是 `Net` 之外的新运行时，专门回答「注意力被谁吸走」：
//! - `scores`：`q · k_i` 的整数分数；
//! - `weights`：softmax 后的注意力分布；
//! - `dominant`：权重最大的 key 下标。

use crate::error::{Error, Result};
use crate::net::Layer;
use crate::trit::Trit;

/// 一个三值注意力头。
#[derive(Clone, Debug, PartialEq)]
pub struct AttentionHead {
    /// Query 投影（`dim → d_k`）。
    pub query: Layer,
    /// Key 投影（`dim → d_k`）。
    pub key: Layer,
    /// Value 投影（`dim → d_v`）。
    pub value: Layer,
}

/// 一次注意力的 trace。
#[derive(Clone, Debug, PartialEq)]
pub struct AttentionTrace {
    /// 每个 key 的整数分数。
    pub scores: Vec<i32>,
    /// softmax 权重（和为 1）。
    pub weights: Vec<f32>,
    /// 加权后的输出（`d_v` 维 f32）。
    pub output: Vec<f32>,
    /// 权重最大的 key 下标。
    pub dominant: usize,
}

impl AttentionHead {
    /// 构造并校验维度：Q/K 的 `rows` 必须相同，三者的 `cols` 必须相同。
    pub fn new(query: Layer, key: Layer, value: Layer) -> Result<Self> {
        if query.rows() != key.rows() {
            return Err(Error::Shape {
                what: "attention d_k",
                expected: query.rows(),
                got: key.rows(),
            });
        }
        if query.cols() != key.cols() || query.cols() != value.cols() {
            return Err(Error::Shape {
                what: "attention dim",
                expected: query.cols(),
                got: value.cols(),
            });
        }
        Ok(Self { query, key, value })
    }

    /// 一次单头注意力：`query` 对 `keys` 做点积，softmax 后加权 `values`。
    pub fn forward(
        &self,
        query: &[Trit],
        keys: &[Vec<Trit>],
        values: &[Vec<Trit>],
    ) -> Result<AttentionTrace> {
        if keys.len() != values.len() {
            return Err(Error::Shape {
                what: "keys/values",
                expected: keys.len(),
                got: values.len(),
            });
        }
        if keys.is_empty() {
            return Err(Error::Shape {
                what: "keys",
                expected: 1,
                got: 0,
            });
        }
        let q = layer_forward(&self.query, query);
        let scores: Vec<i32> = keys
            .iter()
            .map(|key| {
                let k = layer_forward(&self.key, key);
                q.iter()
                    .zip(&k)
                    .map(|(a, b)| i32::from(a.to_i8()) * i32::from(b.to_i8()))
                    .sum()
            })
            .collect();
        let max = scores.iter().copied().max().unwrap_or(0);
        let exps: Vec<f32> = scores
            .iter()
            .map(|score| ((score - max) as f32).exp())
            .collect();
        let total: f32 = exps.iter().sum();
        let weights: Vec<f32> = if total == 0.0 {
            vec![1.0 / keys.len() as f32; keys.len()]
        } else {
            exps.iter().map(|value| value / total).collect()
        };
        let mut output = vec![0.0f32; self.value.rows()];
        for (weight, value) in weights.iter().zip(values) {
            let v = layer_forward(&self.value, value);
            for (slot, trit) in output.iter_mut().zip(&v) {
                *slot += weight * f32::from(trit.to_i8());
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
        Ok(AttentionTrace {
            scores,
            weights,
            output,
            dominant,
        })
    }
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
