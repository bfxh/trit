//! 反向传播（直通估计器 STE）：f32 影子权重，前向用三值量化后的权重。
//!
//! 三值权重不可导，标准做法是保留一份 f32 影子权重 `w`：
//!
//! - 前向：`q = clamp(round(w), -1, 1)`，网络用 `q`；
//! - 反向：把 `dq/dw` 当成 1（直通），梯度直接写回影子权重；
//! - 导出：训练结束后把影子权重四舍五入到 `{-1,0,+1}`。
//!
//! 只支持 1 个隐层的 MLP（`tanh` 激活），输入与标签都是 `±1`。

use crate::model::Model;

/// 训练超参。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainOptions {
    /// 全批梯度下降轮数。
    pub epochs: usize,
    /// 学习率。
    pub learning_rate: f32,
    /// 影子权重初始扰动的种子（确定性）。
    pub seed: u64,
}

impl Default for TrainOptions {
    fn default() -> Self {
        Self {
            epochs: 200,
            learning_rate: 0.2,
            seed: 7,
        }
    }
}

/// 训练报告。
#[derive(Debug, Clone, PartialEq)]
pub struct TrainReport {
    /// 实际轮数。
    pub epochs: usize,
    /// 第一轮的全批损失。
    pub initial_loss: f32,
    /// 最后一轮的全批损失。
    pub final_loss: f32,
    /// 影子权重（未量化）在训练集上的精度。
    pub shadow_accuracy: f32,
    /// 导出成三值网络后的精度。
    pub ternary_accuracy: f32,
}

/// 训练错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrainError {
    /// 模型形状/取值不合法。
    #[error("模型不合法：{0}")]
    Model(#[from] crate::model::ModelError),
    /// 只支持两层模型。
    #[error("训练只支持两层模型（隐层 + 输出层），实际 {0} 层")]
    LayerCount(usize),
    /// 数据为空或维度不符。
    #[error("数据不合法：{0}")]
    Data(String),
    /// 标签不是 ±1。
    #[error("标签必须是 ±1，遇到 {0}")]
    Label(String),
}

/// 多数表决数据集：`n` 个 `±1` 输入，票数过半为 `+1`。
pub fn majority_dataset(n: usize) -> (Vec<Vec<f32>>, Vec<f32>) {
    let total = 1usize << n;
    let mut inputs = Vec::with_capacity(total);
    let mut labels = Vec::with_capacity(total);
    for mask in 0..total {
        let sample: Vec<f32> = (0..n)
            .map(|bit| if (mask >> bit) & 1 == 1 { 1.0 } else { -1.0 })
            .collect();
        let positive = sample.iter().filter(|value| **value > 0.0).count();
        labels.push(if positive * 2 > n { 1.0 } else { -1.0 });
        inputs.push(sample);
    }
    (inputs, labels)
}

fn quantize(value: f32) -> f32 {
    value.round().clamp(-1.0, 1.0)
}

fn tanh(value: f32) -> f32 {
    value.tanh()
}

struct Shadow {
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: f32,
    inputs: usize,
    hidden: usize,
}

impl Shadow {
    fn from_model(model: &Model, seed: u64) -> Result<Self, TrainError> {
        if model.layers.len() != 2 {
            return Err(TrainError::LayerCount(model.layers.len()));
        }
        let first = &model.layers[0];
        let second = &model.layers[1];
        if second.outputs != 1 || second.inputs != first.outputs {
            return Err(TrainError::Data(
                "输出层必须是 1 个神经元且与隐层衔接".to_string(),
            ));
        }
        let mut state = seed | 1;
        let mut noise = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // 映射到 [-0.05, 0.05)
            (state % 1000) as f32 / 10_000.0 - 0.05
        };
        let w1: Vec<f32> = first
            .weights
            .iter()
            .flatten()
            .map(|weight| f32::from(*weight) + noise())
            .collect();
        let b1: Vec<f32> = first
            .bias
            .iter()
            .map(|bias| f32::from(*bias) + noise())
            .collect();
        let w2: Vec<f32> = second
            .weights
            .iter()
            .flatten()
            .map(|weight| f32::from(*weight) + noise())
            .collect();
        Ok(Self {
            w1,
            b1,
            w2,
            b2: second.bias.first().map_or(0.0, |bias| f32::from(*bias)),
            inputs: first.inputs,
            hidden: first.outputs,
        })
    }

    fn forward(&self, sample: &[f32]) -> (Vec<f32>, f32, Vec<f32>) {
        let mut pre = vec![0.0f32; self.hidden];
        for (unit, slot) in pre.iter_mut().enumerate() {
            let mut acc = quantize(self.b1[unit]);
            for (input, value) in sample.iter().enumerate() {
                acc += quantize(self.w1[unit * self.inputs + input]) * value;
            }
            *slot = acc;
        }
        let hidden: Vec<f32> = pre.iter().map(|value| tanh(*value)).collect();
        let mut acc = quantize(self.b2);
        for (unit, value) in hidden.iter().enumerate() {
            acc += quantize(self.w2[unit]) * value;
        }
        (hidden, tanh(acc), pre)
    }
}

/// 训练并把结果写回 `model`（权重会被量化到 `{-1,0,+1}`）。
pub fn train(
    model: &mut Model,
    inputs: &[Vec<f32>],
    labels: &[f32],
    options: TrainOptions,
) -> Result<TrainReport, TrainError> {
    if inputs.is_empty() || inputs.len() != labels.len() {
        return Err(TrainError::Data("输入为空或与标签数量不一致".to_string()));
    }
    // M09：超参数也要显式校验——NaN 学习率会把整张网络变成 NaN，却仍然"训练成功"。
    if !options.learning_rate.is_finite() || options.learning_rate <= 0.0 {
        return Err(TrainError::Data(format!(
            "learning_rate 必须是正的有限值，实际 {}",
            options.learning_rate
        )));
    }
    if options.epochs == 0 {
        return Err(TrainError::Data(
            "epochs 必须至少 1（0 轮会返回虚假的 0 损失并改写权重）".to_string(),
        ));
    }
    // H09：入口先校验模型，再**逐条**校验样本；只查第一条会让后续短样本
    // 在 `Shadow::forward` 里索引 panic。
    model.validate()?;
    for (index, sample) in inputs.iter().enumerate() {
        if sample.len() != model.inputs() {
            return Err(TrainError::Data(format!(
                "第 {index} 条样本维数应为 {}，实际 {}",
                model.inputs(),
                sample.len()
            )));
        }
        if let Some(position) = sample.iter().position(|value| !value.is_finite()) {
            return Err(TrainError::Data(format!(
                "第 {index} 条样本第 {position} 维不是有限值"
            )));
        }
    }
    for label in labels {
        if *label != 1.0 && *label != -1.0 {
            return Err(TrainError::Label(format!("{label}")));
        }
    }
    let mut shadow = Shadow::from_model(model, options.seed)?;
    let mut initial_loss = 0.0f32;
    let mut final_loss = 0.0f32;
    let count = inputs.len() as f32;
    for epoch in 0..options.epochs {
        let mut grad_w1 = vec![0.0f32; shadow.w1.len()];
        let mut grad_b1 = vec![0.0f32; shadow.b1.len()];
        let mut grad_w2 = vec![0.0f32; shadow.w2.len()];
        let mut grad_b2 = 0.0f32;
        let mut loss = 0.0f32;
        for (sample, target) in inputs.iter().zip(labels.iter()) {
            let (hidden, output, pre) = shadow.forward(sample);
            let error = output - target;
            loss += error * error / 2.0;
            let d_output = error * (1.0 - output * output);
            grad_b2 += d_output;
            for (unit, value) in hidden.iter().enumerate() {
                grad_w2[unit] += d_output * value;
            }
            for unit in 0..pre.len() {
                // tanh 的导数：1 - h^2（h 就是 tanh(pre)）。
                let d_hidden =
                    d_output * quantize(shadow.w2[unit]) * (1.0 - hidden[unit] * hidden[unit]);
                grad_b1[unit] += d_hidden;
                for (input, value) in sample.iter().enumerate() {
                    grad_w1[unit * shadow.inputs + input] += d_hidden * value;
                }
            }
        }
        // M09：一旦出现非有限值立刻停，避免把 NaN/Inf 写回模型。
        if !loss.is_finite() {
            return Err(TrainError::Data(format!(
                "第 {epoch} 轮损失不是有限值（训练发散）"
            )));
        }
        if epoch == 0 {
            initial_loss = loss / count;
        }
        final_loss = loss / count;
        let scale = options.learning_rate / count;
        for (weight, gradient) in shadow.w1.iter_mut().zip(grad_w1.iter()) {
            *weight -= scale * gradient;
        }
        for (bias, gradient) in shadow.b1.iter_mut().zip(grad_b1.iter()) {
            *bias -= scale * gradient;
        }
        for (weight, gradient) in shadow.w2.iter_mut().zip(grad_w2.iter()) {
            *weight -= scale * gradient;
        }
        shadow.b2 -= scale * grad_b2;
    }

    let shadow_accuracy = accuracy(&shadow, inputs, labels);
    export(model, &shadow)?;
    let ternary_accuracy = ternary_accuracy(model, inputs, labels);
    Ok(TrainReport {
        epochs: options.epochs,
        initial_loss,
        final_loss,
        shadow_accuracy,
        ternary_accuracy,
    })
}

fn predict(shadow: &Shadow, sample: &[f32]) -> f32 {
    let (_, output, _) = shadow.forward(sample);
    if output >= 0.0 { 1.0 } else { -1.0 }
}

fn accuracy(shadow: &Shadow, inputs: &[Vec<f32>], labels: &[f32]) -> f32 {
    let correct = inputs
        .iter()
        .zip(labels.iter())
        .filter(|(sample, target)| predict(shadow, sample) == **target)
        .count();
    correct as f32 / inputs.len() as f32
}

fn export(model: &mut Model, shadow: &Shadow) -> Result<(), TrainError> {
    let first = &mut model.layers[0];
    for (slot, weight) in first.weights.iter_mut().flatten().zip(shadow.w1.iter()) {
        *slot = quantize(*weight) as i8;
    }
    for (slot, bias) in first.bias.iter_mut().zip(shadow.b1.iter()) {
        *slot = quantize(*bias) as i8;
    }
    let second = &mut model.layers[1];
    for (slot, weight) in second.weights.iter_mut().flatten().zip(shadow.w2.iter()) {
        *slot = quantize(*weight) as i8;
    }
    if let Some(slot) = second.bias.first_mut() {
        *slot = quantize(shadow.b2) as i8;
    }
    model
        .validate()
        .map_err(|error| TrainError::Data(error.to_string()))
}

fn ternary_accuracy(model: &Model, inputs: &[Vec<f32>], labels: &[f32]) -> f32 {
    use crate::infer::{Sign, forward};
    use trit_core::value::Ternary;
    let mut correct = 0;
    for (sample, target) in inputs.iter().zip(labels.iter()) {
        let values: Vec<Ternary> = sample
            .iter()
            .map(|value| {
                if *value > 0.0 {
                    Ternary::True
                } else {
                    Ternary::False
                }
            })
            .collect();
        let Ok(output) = forward(model, &values) else {
            continue;
        };
        let predicted = match output.first() {
            Some(Sign::Pos) => 1.0,
            Some(Sign::Neg) => -1.0,
            // Zero（弃权）按错误计。
            _ => 0.0,
        };
        if predicted == *target {
            correct += 1;
        }
    }
    correct as f32 / inputs.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn untrained(hidden: usize, inputs: usize) -> Model {
        let mut model = Model {
            layers: vec![
                crate::model::Layer {
                    inputs,
                    outputs: hidden,
                    weights: vec![vec![0; inputs]; hidden],
                    bias: vec![0; hidden],
                },
                crate::model::Layer {
                    inputs: hidden,
                    outputs: 1,
                    weights: vec![vec![0; hidden]],
                    bias: vec![0],
                },
            ],
        };
        // 用固定种子给一个非对称初值，避免全 0。
        for (index, weight) in model.layers[0].weights.iter_mut().flatten().enumerate() {
            *weight = if index % 3 == 0 { 1 } else { -1 };
        }
        for (index, weight) in model.layers[1].weights.iter_mut().flatten().enumerate() {
            *weight = if index % 2 == 0 { 1 } else { -1 };
        }
        model
    }

    #[test]
    fn majority_five_is_learned() {
        let (inputs, labels) = majority_dataset(5);
        let mut model = untrained(6, 5);
        let report = train(
            &mut model,
            &inputs,
            &labels,
            TrainOptions {
                epochs: 300,
                learning_rate: 0.3,
                seed: 11,
            },
        )
        .unwrap();
        assert!(report.final_loss < report.initial_loss, "损失没有下降");
        assert!(
            report.ternary_accuracy >= 0.75,
            "三值网络精度只有 {}",
            report.ternary_accuracy
        );
    }

    #[test]
    fn rejects_non_finite_learning_rate_and_zero_epochs() {
        // 复审 M09：NaN 学习率曾让报告整体变 NaN 仍算"成功"。
        let (inputs, labels) = majority_dataset(3);
        let mut model = untrained(4, 3);
        let error = train(
            &mut model,
            &inputs,
            &labels,
            TrainOptions {
                learning_rate: f32::NAN,
                ..TrainOptions::default()
            },
        );
        assert!(
            matches!(&error, Err(TrainError::Data(message)) if message.contains("learning_rate")),
            "NaN 学习率必须拒绝，实际 {error:?}"
        );
        let error = train(
            &mut model,
            &inputs,
            &labels,
            TrainOptions {
                learning_rate: 0.0,
                ..TrainOptions::default()
            },
        );
        assert!(
            matches!(error, Err(TrainError::Data(_))),
            "0 学习率必须拒绝"
        );
        let error = train(
            &mut model,
            &inputs,
            &labels,
            TrainOptions {
                epochs: 0,
                ..TrainOptions::default()
            },
        );
        assert!(
            matches!(&error, Err(TrainError::Data(message)) if message.contains("epochs")),
            "0 轮必须拒绝，实际 {error:?}"
        );
    }

    #[test]
    fn rejects_inconsistent_model_and_late_short_samples() {
        // 复审 H09：只查第一条样本，第二条更短曾在 `Shadow::forward` 索引 panic。
        let (inputs, labels) = majority_dataset(3);
        let mut model = untrained(4, 3);
        let mut samples = inputs.clone();
        samples[1] = vec![1.0];
        let error = train(&mut model, &samples, &labels, TrainOptions::default());
        assert!(
            matches!(&error, Err(TrainError::Data(message)) if message.contains("第 1 条")),
            "短样本应在训练前被拒，实际 {error:?}"
        );

        // 手搓两层模型：隐层 outputs=2 但 weights 为空 ⇒ 模型校验错误，不是 panic。
        // 用两层而不是一层，是为了让「去掉入口 validate」真的落到索引 panic 上，
        // 而不是先被 LayerCount 拦下。
        let mut broken = Model {
            layers: vec![
                crate::model::Layer {
                    inputs: 2,
                    outputs: 2,
                    weights: Vec::new(),
                    bias: vec![0, 0],
                },
                crate::model::Layer {
                    inputs: 2,
                    outputs: 1,
                    weights: vec![vec![1, 1]],
                    bias: vec![0],
                },
            ],
        };
        let error = train(
            &mut broken,
            &[vec![1.0, -1.0]],
            &[1.0],
            TrainOptions::default(),
        );
        assert!(
            matches!(error, Err(TrainError::Model(_))),
            "非法模型应在训练前被拒，实际 {error:?}"
        );

        // 非有限输入会让报告变成 NaN，同样在入口拒绝。
        let mut model = untrained(4, 3);
        let mut samples = inputs.clone();
        samples[0][0] = f32::NAN;
        let error = train(&mut model, &samples, &labels, TrainOptions::default());
        assert!(
            matches!(&error, Err(TrainError::Data(message)) if message.contains("有限值")),
            "非有限样本应被拒，实际 {error:?}"
        );
    }

    #[test]
    fn labels_must_be_pm_one() {
        let (inputs, _) = majority_dataset(3);
        let mut model = untrained(4, 3);
        let error = train(
            &mut model,
            &inputs,
            &vec![0.5; inputs.len()],
            TrainOptions::default(),
        );
        assert!(matches!(error, Err(TrainError::Label(_))));
    }

    #[test]
    fn layer_count_is_checked() {
        let mut model = Model {
            layers: vec![crate::model::Layer {
                inputs: 2,
                outputs: 1,
                weights: vec![vec![1, 0]],
                bias: vec![0],
            }],
        };
        let error = train(
            &mut model,
            &[vec![1.0, -1.0]],
            &[1.0],
            TrainOptions::default(),
        );
        assert!(matches!(error, Err(TrainError::LayerCount(1))));
    }
}
