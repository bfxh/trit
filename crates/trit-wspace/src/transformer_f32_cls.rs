//! 分类头 + softmax 交叉熵：接在 [`crate::transformer_f32_full`] 的全参数反向上。
//!
//! 每个 token 是一个样本（上下文仍是整段 token），特征是它自己的块输出；
//! 损失是批平均交叉熵，梯度同时进入分类头和 block 的全部参数。
//!
//! 与 MSE 的关键差别：交叉熵对**尺度不敏感**（logits 整体平移/缩放会被 softmax 归一），
//! 所以准确率上升是"学到了方向"的证据，而不是"把输出压小对齐模长"。

use crate::error::{Error, Result};
use crate::transformer_f32::TransformerBlock;
use crate::transformer_f32_full::Grads;
use crate::transformer_f32_full::step::{backward_token, forward_all};

/// 线性分类头：`logits = weight · features + bias`，`weight` 形状 `classes × dim`。
#[derive(Clone, Debug, PartialEq)]
pub struct ClassifierHead {
    weight: Vec<f32>,
    bias: Vec<f32>,
    classes: usize,
    dim: usize,
}

impl ClassifierHead {
    /// 全零初始化（权重与偏置都是 0）。
    pub fn zeros(classes: usize, dim: usize) -> Result<Self> {
        if classes == 0 || dim == 0 {
            return Err(Error::EmptyLayer {
                rows: classes,
                cols: dim,
            });
        }
        Ok(Self {
            weight: vec![0.0; classes * dim],
            bias: vec![0.0; classes],
            classes,
            dim,
        })
    }

    /// 直接给权重（`weight` 行主序 `classes × dim`，`bias` 长度 `classes`）。
    pub fn new(weight: Vec<f32>, bias: Vec<f32>, classes: usize, dim: usize) -> Result<Self> {
        if weight.len() != classes * dim {
            return Err(Error::Shape {
                what: "classifier weight",
                expected: classes * dim,
                got: weight.len(),
            });
        }
        if bias.len() != classes {
            return Err(Error::Shape {
                what: "classifier bias",
                expected: classes,
                got: bias.len(),
            });
        }
        Ok(Self {
            weight,
            bias,
            classes,
            dim,
        })
    }

    /// 类别数。
    #[must_use]
    pub fn classes(&self) -> usize {
        self.classes
    }

    /// 特征维数。
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 权重（行主序 `classes × dim`）。
    #[must_use]
    pub fn weight(&self) -> &[f32] {
        &self.weight
    }

    /// 偏置。
    #[must_use]
    pub fn bias(&self) -> &[f32] {
        &self.bias
    }

    /// 一个样本的预测类别。
    pub fn predict(&self, features: &[f32]) -> Result<usize> {
        let logits = self.logits(features)?;
        Ok(argmax(&logits))
    }

    fn logits(&self, features: &[f32]) -> Result<Vec<f32>> {
        if features.len() != self.dim {
            return Err(Error::Shape {
                what: "classifier input",
                expected: self.dim,
                got: features.len(),
            });
        }
        Ok((0..self.classes)
            .map(|class| {
                let line = &self.weight[class * self.dim..(class + 1) * self.dim];
                line.iter()
                    .zip(features)
                    .map(|(weight, value)| weight * value)
                    .sum::<f32>()
                    + self.bias[class]
            })
            .collect())
    }
}

/// 分类训练选项。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassifyOptions {
    /// 轮数。
    pub rounds: usize,
    /// block 参数的学习率。
    pub lr_block: f32,
    /// 分类头的学习率。
    pub lr_head: f32,
}

/// 分类训练报告。
#[derive(Clone, Debug, PartialEq)]
pub struct ClassifyReport {
    /// 每轮批平均交叉熵。
    pub losses: Vec<f32>,
    /// 每轮准确率（`argmax` 命中比例）。
    pub accuracies: Vec<f32>,
}

/// 用交叉熵训练分类头与 block 全参数。
///
/// `tokens[i]` 是第 i 个样本，同时整段 `tokens` 充当 attention 的 K/V 上下文；
/// `labels[i]` 必须在 `0..head.classes()` 内。
pub fn train_classify(
    block: &mut TransformerBlock,
    head: &mut ClassifierHead,
    tokens: &[Vec<f32>],
    labels: &[usize],
    options: &ClassifyOptions,
) -> Result<ClassifyReport> {
    if tokens.len() != labels.len() {
        return Err(Error::Shape {
            what: "labels",
            expected: tokens.len(),
            got: labels.len(),
        });
    }
    if tokens.is_empty() {
        return Err(Error::Shape {
            what: "samples",
            expected: 1,
            got: 0,
        });
    }
    for (index, label) in labels.iter().enumerate() {
        if *label >= head.classes {
            return Err(Error::OutOfBounds {
                layer: index,
                row: *label,
                col: head.classes,
            });
        }
    }
    let count = tokens.len() as f32;
    let mut losses = Vec::with_capacity(options.rounds);
    let mut accuracies = Vec::with_capacity(options.rounds);
    for _ in 0..options.rounds {
        let caches = forward_all(block, tokens)?;
        let mut total = 0.0f32;
        let mut hits = 0usize;
        let mut grads = Grads::default();
        let mut grad_weight = vec![0.0f32; head.weight.len()];
        let mut grad_bias = vec![0.0f32; head.classes];
        for ((cache, label), token) in caches.iter().zip(labels).zip(tokens) {
            let features = cache.output();
            let logits = head.logits(features)?;
            let probs = softmax(&logits);
            total += -probs[*label].max(1e-12).ln();
            if argmax(&probs) == *label {
                hits += 1;
            }
            let mut grad_logits = probs;
            grad_logits[*label] -= 1.0;
            for class in 0..head.classes {
                let scaled = grad_logits[class] / count;
                grad_bias[class] += scaled;
                let line = &mut grad_weight[class * head.dim..(class + 1) * head.dim];
                for (slot, feature) in line.iter_mut().zip(features) {
                    *slot += scaled * feature;
                }
            }
            let grad_x2: Vec<f32> = (0..head.dim)
                .map(|dim| {
                    let sum: f32 = (0..head.classes)
                        .map(|class| head.weight[class * head.dim + dim] * grad_logits[class])
                        .sum();
                    sum / count
                })
                .collect();
            backward_token(block, token, tokens, cache, &grad_x2, &mut grads);
        }
        // 交叉熵已是批平均，梯度不再二次除以样本数。
        grads.apply(block, options.lr_block, 1.0);
        for (slot, gradient) in head.weight.iter_mut().zip(&grad_weight) {
            *slot -= options.lr_head * gradient;
        }
        for (slot, gradient) in head.bias.iter_mut().zip(&grad_bias) {
            *slot -= options.lr_head * gradient;
        }
        losses.push(total / count);
        accuracies.push(hits as f32 / count);
    }
    Ok(ClassifyReport { losses, accuracies })
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|value| (value - max).exp()).collect();
    let total: f32 = exps.iter().sum();
    exps.iter().map(|value| value / total).collect()
}

fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}
