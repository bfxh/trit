//! 分类训练：softmax 交叉熵的梯度同时回传到分类头与 block 全部参数。
//!
//! 头这个工件（以及闭式岭回归）在 [`crate::classifier_head`]，这里只做 SGD 训练循环。
//!
//! 两个入口：
//! - [`train_classify`]：样本集**同时充当** attention 上下文（简明，代价是 N 个样本就是
//!   N×N 注意力，样本数一大就跑不动）；
//! - [`train_classify_with`]：样本与上下文分开，可以用小上下文 + 大样本集做留出评估。
//!
//! 与 MSE 的关键差别：交叉熵对尺度不敏感（logits 整体平移/缩放会被 softmax 归一），
//! 所以准确率上升才是"学到方向"的证据，而不是"把输出压小对齐模长"。

pub use crate::classifier_head::ClassifierHead;
use crate::error::{Error, Result};
use crate::grads::Grads;
use crate::transformer_f32::TransformerBlock;
use crate::transformer_f32_full::step::{TokenCache, backward_token, forward_token};

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
    /// 每轮训练集准确率（`argmax` 命中比例）。
    pub accuracies: Vec<f32>,
}

/// 样本与上下文分开的推理：`samples[i]` 只对 `context` 做注意力，返回块输出。
pub fn forward_with(
    block: &TransformerBlock,
    samples: &[Vec<f32>],
    context: &[Vec<f32>],
) -> Result<Vec<Vec<f32>>> {
    samples
        .iter()
        .map(|sample| forward_token(block, sample, context).map(|cache| cache.output().to_vec()))
        .collect()
}

/// 样本集同时充当 attention 上下文的简写。
pub fn train_classify(
    block: &mut TransformerBlock,
    head: &mut ClassifierHead,
    tokens: &[Vec<f32>],
    labels: &[usize],
    options: &ClassifyOptions,
) -> Result<ClassifyReport> {
    train_classify_with(block, head, tokens, tokens, labels, options)
}

/// 用交叉熵训练分类头与 block 全参数；`samples[i]` 只对 `context` 做注意力。
pub fn train_classify_with(
    block: &mut TransformerBlock,
    head: &mut ClassifierHead,
    samples: &[Vec<f32>],
    context: &[Vec<f32>],
    labels: &[usize],
    options: &ClassifyOptions,
) -> Result<ClassifyReport> {
    if samples.len() != labels.len() {
        return Err(Error::Shape {
            what: "labels",
            expected: samples.len(),
            got: labels.len(),
        });
    }
    if samples.is_empty() || context.is_empty() {
        return Err(Error::Shape {
            what: "samples",
            expected: 1,
            got: samples.len().min(context.len()),
        });
    }
    for (index, label) in labels.iter().enumerate() {
        if *label >= head.classes() {
            return Err(Error::OutOfBounds {
                layer: index,
                row: *label,
                col: head.classes(),
            });
        }
    }
    let classes = head.classes();
    let head_dim = head.dim();
    let count = samples.len() as f32;
    let mut losses = Vec::with_capacity(options.rounds);
    let mut accuracies = Vec::with_capacity(options.rounds);
    for _ in 0..options.rounds {
        let caches: Vec<TokenCache> = samples
            .iter()
            .map(|sample| forward_token(block, sample, context))
            .collect::<Result<_>>()?;
        let mut total = 0.0f32;
        let mut hits = 0usize;
        let mut grads = Grads::default();
        let mut grad_weight = vec![0.0f32; classes * head_dim];
        let mut grad_bias = vec![0.0f32; classes];
        for ((cache, label), sample) in caches.iter().zip(labels).zip(samples) {
            let features = cache.output();
            let probs = softmax(&head.logits(features)?);
            total += -probs[*label].max(1e-12).ln();
            if argmax(&probs) == *label {
                hits += 1;
            }
            let mut grad_logits = probs;
            grad_logits[*label] -= 1.0;
            for class in 0..classes {
                let scaled = grad_logits[class] / count;
                grad_bias[class] += scaled;
                let line = &mut grad_weight[class * head_dim..(class + 1) * head_dim];
                for (slot, feature) in line.iter_mut().zip(features) {
                    *slot += scaled * feature;
                }
            }
            let grad_x2: Vec<f32> = (0..head_dim)
                .map(|dim| {
                    let sum: f32 = (0..classes)
                        .map(|class| head.weight()[class * head_dim + dim] * grad_logits[class])
                        .sum();
                    sum / count
                })
                .collect();
            backward_token(block, sample, context, cache, &grad_x2, &mut grads);
        }
        // 交叉熵已是批平均，梯度不再二次除以样本数。
        grads.apply(block, options.lr_block, 1.0);
        for (slot, gradient) in head.weight_mut().iter_mut().zip(&grad_weight) {
            *slot -= options.lr_head * gradient;
        }
        for (slot, gradient) in head.bias_mut().iter_mut().zip(&grad_bias) {
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
