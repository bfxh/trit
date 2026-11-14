//! Adam 版的分类训练：交叉熵梯度同时更新分类头与块的全部参数。
//!
//! 为什么不是 SGD：[`crate::transformer_f32_cls::train_classify_with`] 用 plain SGD，
//! 在 384 维 LayerNorm 输出上条件数很差——实测 200 步后训练集准确率只有 0.40，
//! 而同一批特征闭式岭回归能到 0.89。Adam 用二阶矩做逐参数缩放，对特征尺度不敏感，
//! 是这个病根的对症解法。
//!
//! 接口是**新增**的（`train_classify_adam_with`），不改动已提交的 SGD 版本。

use crate::classifier_head::ClassifierHead;
use crate::error::{Error, Result};
use crate::grads::{AdamCfg, BlockAdam, Grads, TensorAdam};
use crate::transformer_f32::TransformerBlock;
use crate::transformer_f32_cls::ClassifyReport;
use crate::transformer_f32_full::step::{TokenCache, backward_token, forward_token};

/// Adam 训练选项。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdamOptions {
    /// 轮数（每轮是一个小批量）。
    pub rounds: usize,
    /// 块参数学习率。
    pub lr_block: f32,
    /// 分类头学习率。
    pub lr_head: f32,
    /// 一阶矩衰减。
    pub beta1: f32,
    /// 二阶矩衰减。
    pub beta2: f32,
    /// 数值稳定项。
    pub eps: f32,
}

impl Default for AdamOptions {
    fn default() -> Self {
        Self {
            rounds: 200,
            lr_block: 1e-3,
            lr_head: 1e-2,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
        }
    }
}

impl AdamOptions {
    fn block_cfg(&self) -> AdamCfg {
        AdamCfg {
            lr: self.lr_block,
            beta1: self.beta1,
            beta2: self.beta2,
            eps: self.eps,
        }
    }

    fn head_cfg(&self) -> AdamCfg {
        AdamCfg {
            lr: self.lr_head,
            beta1: self.beta1,
            beta2: self.beta2,
            eps: self.eps,
        }
    }
}

/// Adam 的**跨步**状态（一阶/二阶矩 + 步数）。
///
/// 小批量训练必须复用同一个状态：每步重建会让动量和偏置校正归零，
/// Adam 退化成近似的 sign-SGD，loss 会抖动甚至上升（实测过）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdamState {
    block: BlockAdam,
    weight: TensorAdam,
    bias: TensorAdam,
    t: u32,
}

impl AdamState {
    /// 按当前块/头形状建状态。
    #[must_use]
    pub fn new(block: &TransformerBlock, head: &ClassifierHead) -> Self {
        Self {
            block: BlockAdam::new(block),
            weight: TensorAdam::new(head.classes() * head.dim()),
            bias: TensorAdam::new(head.classes()),
            t: 0,
        }
    }

    /// 已经走过的步数。
    #[must_use]
    pub fn steps(&self) -> u32 {
        self.t
    }
}

/// 用 Adam 训练分类头与块全参数；`samples[i]` 只对 `context` 做注意力。
pub fn train_classify_adam_with(
    block: &mut TransformerBlock,
    head: &mut ClassifierHead,
    samples: &[Vec<f32>],
    context: &[Vec<f32>],
    labels: &[usize],
    options: &AdamOptions,
) -> Result<ClassifyReport> {
    let mut state = AdamState::new(block, head);
    train_classify_adam_with_state(block, head, samples, context, labels, options, &mut state)
}

/// 同上，但复用调用方给的 [`AdamState`]——小批量循环必须走这个入口。
pub fn train_classify_adam_with_state(
    block: &mut TransformerBlock,
    head: &mut ClassifierHead,
    samples: &[Vec<f32>],
    context: &[Vec<f32>],
    labels: &[usize],
    options: &AdamOptions,
    state: &mut AdamState,
) -> Result<ClassifyReport> {
    validate(head, samples, context, labels)?;
    let classes = head.classes();
    let head_dim = head.dim();
    let count = samples.len() as f32;
    let block_cfg = options.block_cfg();
    let head_cfg = options.head_cfg();
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
        state.t = state.t.saturating_add(1);
        let step = state.t as f32;
        state.block.step(block, &grads, &block_cfg, step, count);
        state
            .weight
            .step(head.weight_mut(), &grad_weight, &head_cfg, step);
        state
            .bias
            .step(head.bias_mut(), &grad_bias, &head_cfg, step);
        losses.push(total / count);
        accuracies.push(hits as f32 / count);
    }
    Ok(ClassifyReport { losses, accuracies })
}

fn validate(
    head: &ClassifierHead,
    samples: &[Vec<f32>],
    context: &[Vec<f32>],
    labels: &[usize],
) -> Result<()> {
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
    Ok(())
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
