//! 梯度累加与参数更新：SGD（原来在 `transformer_f32_full` 里）与 **Adam**。
//!
//! 为什么要 Adam：块输出经过两层 LayerNorm，384 维特征的每维尺度小且条件数差，
//! plain SGD 在同一预算下连训练集都拟合不上（实测 200 步只到 0.40，闭式解 0.89）。
//! Adam 用一阶梯度的二阶矩做逐参数缩放，对特征尺度不敏感，正是这种情形的解药。

use crate::transformer_f32::TransformerBlock;

/// 全部块参数上的梯度累加器。
#[derive(Default)]
pub(crate) struct Grads {
    pub(crate) query: Vec<f32>,
    pub(crate) key: Vec<f32>,
    pub(crate) value: Vec<f32>,
    pub(crate) ln1_weight: Vec<f32>,
    pub(crate) ln1_bias: Vec<f32>,
    pub(crate) ln2_weight: Vec<f32>,
    pub(crate) ln2_bias: Vec<f32>,
    pub(crate) ff1_weight: Vec<f32>,
    pub(crate) ff1_bias: Vec<f32>,
    pub(crate) ff2_weight: Vec<f32>,
    pub(crate) ff2_bias: Vec<f32>,
}

impl Grads {
    /// 累加一次单 token 的反向结果。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add(
        &mut self,
        attention: &crate::transformer_f32_full::AttentionGrad,
        gw_ln1: &[f32],
        gb_ln1: &[f32],
        gw_ln2: &[f32],
        gb_ln2: &[f32],
        gw_ff1: &[f32],
        gb_ff1: &[f32],
        gw_ff2: &[f32],
        gb_ff2: &[f32],
    ) {
        accumulate(&mut self.query, &attention.query);
        accumulate(&mut self.key, &attention.key);
        accumulate(&mut self.value, &attention.value);
        accumulate(&mut self.ln1_weight, gw_ln1);
        accumulate(&mut self.ln1_bias, gb_ln1);
        accumulate(&mut self.ln2_weight, gw_ln2);
        accumulate(&mut self.ln2_bias, gb_ln2);
        accumulate(&mut self.ff1_weight, gw_ff1);
        accumulate(&mut self.ff1_bias, gb_ff1);
        accumulate(&mut self.ff2_weight, gw_ff2);
        accumulate(&mut self.ff2_bias, gb_ff2);
    }

    /// SGD 更新：`p -= lr * g / count`。
    pub(crate) fn apply(&self, block: &mut TransformerBlock, lr: f32, count: f32) {
        sgd(&mut block.attention.query, &self.query, lr, count);
        sgd(&mut block.attention.key, &self.key, lr, count);
        sgd(&mut block.attention.value, &self.value, lr, count);
        sgd(&mut block.ln1.weight, &self.ln1_weight, lr, count);
        sgd(&mut block.ln1.bias, &self.ln1_bias, lr, count);
        sgd(&mut block.ln2.weight, &self.ln2_weight, lr, count);
        sgd(&mut block.ln2.bias, &self.ln2_bias, lr, count);
        sgd(&mut block.ff1.weight, &self.ff1_weight, lr, count);
        sgd(&mut block.ff1.bias, &self.ff1_bias, lr, count);
        sgd(&mut block.ff2.weight, &self.ff2_weight, lr, count);
        sgd(&mut block.ff2.bias, &self.ff2_bias, lr, count);
    }
}

fn accumulate(target: &mut Vec<f32>, source: &[f32]) {
    if target.len() != source.len() {
        *target = vec![0.0; source.len()];
    }
    for (slot, value) in target.iter_mut().zip(source) {
        *slot += value;
    }
}

fn sgd(target: &mut [f32], gradient: &[f32], lr: f32, count: f32) {
    for (slot, value) in target.iter_mut().zip(gradient) {
        *slot -= lr * value / count;
    }
}

/// Adam 超参。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdamCfg {
    /// 学习率。
    pub lr: f32,
    /// 一阶矩衰减。
    pub beta1: f32,
    /// 二阶矩衰减。
    pub beta2: f32,
    /// 数值稳定项。
    pub eps: f32,
}

impl Default for AdamCfg {
    fn default() -> Self {
        Self {
            lr: 1e-2,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
        }
    }
}

/// 单个张量的 Adam 状态（`m` 一阶矩、`v` 二阶矩）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TensorAdam {
    m: Vec<f32>,
    v: Vec<f32>,
}

impl TensorAdam {
    /// 按参数长度初始化。
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            m: vec![0.0; len],
            v: vec![0.0; len],
        }
    }

    /// 一步 Adam 更新；`t` 是第几步（从 1 开始，用于偏置校正）。
    pub fn step(&mut self, param: &mut [f32], grad: &[f32], cfg: &AdamCfg, t: f32) {
        self.step_scaled(param, grad, cfg, t, 1.0);
    }

    /// 同上，但把梯度按 `count` 归一（批平均）；在循环内做，避免每步分配临时张量。
    pub(crate) fn step_scaled(
        &mut self,
        param: &mut [f32],
        grad: &[f32],
        cfg: &AdamCfg,
        t: f32,
        count: f32,
    ) {
        if self.m.len() != param.len() {
            self.m = vec![0.0; param.len()];
            self.v = vec![0.0; param.len()];
        }
        let bias1 = 1.0 - cfg.beta1.powf(t);
        let bias2 = 1.0 - cfg.beta2.powf(t);
        let scale = if count == 0.0 { 1.0 } else { count };
        for (index, slot) in param.iter_mut().enumerate() {
            let g = grad.get(index).copied().unwrap_or(0.0) / scale;
            self.m[index] = cfg.beta1 * self.m[index] + (1.0 - cfg.beta1) * g;
            self.v[index] = cfg.beta2 * self.v[index] + (1.0 - cfg.beta2) * g * g;
            let m_hat = self.m[index] / bias1;
            let v_hat = self.v[index] / bias2;
            *slot -= cfg.lr * m_hat / (v_hat.sqrt() + cfg.eps);
        }
    }
}

/// 块的全部参数各配一个 Adam 状态。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockAdam {
    query: TensorAdam,
    key: TensorAdam,
    value: TensorAdam,
    ln1_weight: TensorAdam,
    ln1_bias: TensorAdam,
    ln2_weight: TensorAdam,
    ln2_bias: TensorAdam,
    ff1_weight: TensorAdam,
    ff1_bias: TensorAdam,
    ff2_weight: TensorAdam,
    ff2_bias: TensorAdam,
}

impl BlockAdam {
    /// 按 `block` 现有形状建状态。
    #[must_use]
    pub fn new(block: &TransformerBlock) -> Self {
        Self {
            query: TensorAdam::new(block.attention.query.len()),
            key: TensorAdam::new(block.attention.key.len()),
            value: TensorAdam::new(block.attention.value.len()),
            ln1_weight: TensorAdam::new(block.ln1.weight.len()),
            ln1_bias: TensorAdam::new(block.ln1.bias.len()),
            ln2_weight: TensorAdam::new(block.ln2.weight.len()),
            ln2_bias: TensorAdam::new(block.ln2.bias.len()),
            ff1_weight: TensorAdam::new(block.ff1.weight.len()),
            ff1_bias: TensorAdam::new(block.ff1.bias.len()),
            ff2_weight: TensorAdam::new(block.ff2.weight.len()),
            ff2_bias: TensorAdam::new(block.ff2.bias.len()),
        }
    }

    /// 用累加好的梯度走一步 Adam。
    pub(crate) fn step(
        &mut self,
        block: &mut TransformerBlock,
        grads: &Grads,
        cfg: &AdamCfg,
        t: f32,
        count: f32,
    ) {
        self.query
            .step_scaled(&mut block.attention.query, &grads.query, cfg, t, count);
        self.key
            .step_scaled(&mut block.attention.key, &grads.key, cfg, t, count);
        self.value
            .step_scaled(&mut block.attention.value, &grads.value, cfg, t, count);
        self.ln1_weight
            .step_scaled(&mut block.ln1.weight, &grads.ln1_weight, cfg, t, count);
        self.ln1_bias
            .step_scaled(&mut block.ln1.bias, &grads.ln1_bias, cfg, t, count);
        self.ln2_weight
            .step_scaled(&mut block.ln2.weight, &grads.ln2_weight, cfg, t, count);
        self.ln2_bias
            .step_scaled(&mut block.ln2.bias, &grads.ln2_bias, cfg, t, count);
        self.ff1_weight
            .step_scaled(&mut block.ff1.weight, &grads.ff1_weight, cfg, t, count);
        self.ff1_bias
            .step_scaled(&mut block.ff1.bias, &grads.ff1_bias, cfg, t, count);
        self.ff2_weight
            .step_scaled(&mut block.ff2.weight, &grads.ff2_weight, cfg, t, count);
        self.ff2_bias
            .step_scaled(&mut block.ff2.bias, &grads.ff2_bias, cfg, t, count);
    }
}
