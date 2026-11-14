//! 完整 f32 transformer 层：注意力 + 残差 + LayerNorm + FFN（forward-only）。
//!
//! 结构与 MiniLM 的一层一致：`x = LN(x + attn(x))`、`x = LN(x + FFN(x))`。

use crate::attention_f32::F32Attention;
use crate::error::{Error, Result};

/// LayerNorm。
#[derive(Clone, Debug, PartialEq)]
pub struct LayerNorm {
    /// 缩放。
    pub weight: Vec<f32>,
    /// 偏置。
    pub bias: Vec<f32>,
    /// 数值稳定项。
    pub eps: f32,
}

impl LayerNorm {
    /// `weight` 与 `bias` 必须等长且非空。
    pub fn new(weight: Vec<f32>, bias: Vec<f32>) -> Result<Self> {
        if weight.is_empty() || weight.len() != bias.len() {
            return Err(Error::Shape {
                what: "layernorm",
                expected: weight.len().max(1),
                got: bias.len(),
            });
        }
        Ok(Self {
            weight,
            bias,
            eps: 1e-5,
        })
    }

    /// 逐元素归一化。
    #[must_use]
    pub fn forward(&self, x: &[f32]) -> Vec<f32> {
        let count = x.len() as f32;
        let mean = x.iter().sum::<f32>() / count;
        let variance = x
            .iter()
            .map(|value| (value - mean) * (value - mean))
            .sum::<f32>()
            / count;
        let denominator = (variance + self.eps).sqrt();
        x.iter()
            .enumerate()
            .map(|(index, value)| {
                (value - mean) / denominator * self.weight[index] + self.bias[index]
            })
            .collect()
    }
}

/// 全连接层。
#[derive(Clone, Debug, PartialEq)]
pub struct Dense {
    /// 权重（`rows × cols`）。
    pub weight: Vec<f32>,
    /// 偏置。
    pub bias: Vec<f32>,
    /// 输出维。
    pub rows: usize,
    /// 输入维。
    pub cols: usize,
}

impl Dense {
    /// 形状必须匹配。
    pub fn new(weight: Vec<f32>, bias: Vec<f32>, rows: usize, cols: usize) -> Result<Self> {
        if weight.len() != rows * cols || bias.len() != rows {
            return Err(Error::Shape {
                what: "dense",
                expected: rows * cols,
                got: weight.len(),
            });
        }
        Ok(Self {
            weight,
            bias,
            rows,
            cols,
        })
    }

    /// 可选 ReLU。
    #[must_use]
    pub fn forward(&self, x: &[f32], relu: bool) -> Vec<f32> {
        (0..self.rows)
            .map(|row| {
                let line = &self.weight[row * self.cols..(row + 1) * self.cols];
                let sum = line
                    .iter()
                    .zip(x)
                    .map(|(weight, value)| weight * value)
                    .sum::<f32>()
                    + self.bias[row];
                if relu { sum.max(0.0) } else { sum }
            })
            .collect()
    }
}

/// 一个 transformer 块。
#[derive(Clone, Debug, PartialEq)]
pub struct TransformerBlock {
    /// 注意力。
    pub attention: F32Attention,
    /// 注意力后的 LayerNorm。
    pub ln1: LayerNorm,
    /// FFN 第一层。
    pub ff1: Dense,
    /// FFN 第二层。
    pub ff2: Dense,
    /// FFN 后的 LayerNorm。
    pub ln2: LayerNorm,
}

impl TransformerBlock {
    /// 组装一个块。
    pub fn new(
        attention: F32Attention,
        ln1: LayerNorm,
        ff1: Dense,
        ff2: Dense,
        ln2: LayerNorm,
    ) -> Result<Self> {
        Ok(Self {
            attention,
            ln1,
            ff1,
            ff2,
            ln2,
        })
    }

    /// 对一串 token 跑 `attention → 残差 → LN → FFN → 残差 → LN`。
    pub fn forward(&self, tokens: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(tokens.len());
        for token in tokens {
            let attention = self.attention.forward(token, tokens, tokens)?.output;
            let mut x1: Vec<f32> = token
                .iter()
                .zip(&attention)
                .map(|(left, right)| left + right)
                .collect();
            x1 = self.ln1.forward(&x1);
            let hidden = self.ff1.forward(&x1, true);
            let ffn = self.ff2.forward(&hidden, false);
            let mut x2: Vec<f32> = x1
                .iter()
                .zip(&ffn)
                .map(|(left, right)| left + right)
                .collect();
            x2 = self.ln2.forward(&x2);
            out.push(x2);
        }
        Ok(out)
    }
}
