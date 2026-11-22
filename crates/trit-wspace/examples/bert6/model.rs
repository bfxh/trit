//! MiniLM BERT 6 层的 f32 / 三值前向（只三值化线性权重，LN/GELU/softmax 保持 f32）。

use trit_wspace::safetensors;

use crate::activation::{ActivationMode, LayerScales, NormKind, quantize_activation};

use crate::linear::{F32Linear, Linear, QuantLinear};
pub(crate) struct BertLayer {
    query: Linear,
    key: Linear,
    value: Linear,
    output: Linear,
    norm1: NormKind,
    intermediate: Linear,
    ffn_output: Linear,
    norm2: NormKind,
    activation: ActivationMode,
}

pub(crate) struct BertConfig {
    pub(crate) layers: usize,
    pub(crate) hidden: usize,
    pub(crate) intermediate: usize,
    pub(crate) heads: usize,
    pub(crate) eps: f32,
    pub(crate) quantize: bool,
    pub(crate) activation: ActivationMode,
}

pub(crate) struct Bert {
    hidden: usize,
    heads: usize,
    head_dim: usize,
    layers: Vec<BertLayer>,
    scales: Option<Vec<LayerScales>>,
}

impl Bert {
    pub(crate) fn load(file: &str, config: BertConfig) -> Self {
        let BertConfig {
            layers,
            hidden,
            intermediate,
            heads,
            eps,
            quantize,
            activation,
        } = config;
        let tensor = |name: &str| -> F32Linear {
            let weight = safetensors::read_tensor(file, name)
                .unwrap_or_else(|e| panic!("读 {name} 失败：{e}"));
            let bias_name = name.replace(".weight", ".bias");
            let bias = safetensors::read_vector(file, &bias_name)
                .unwrap_or_else(|e| panic!("读 {bias_name} 失败：{e}"));
            F32Linear {
                rows: weight.rows,
                cols: weight.cols,
                weight: weight.values,
                bias,
            }
        };
        let norm = |prefix: &str| -> NormKind {
            let weight =
                safetensors::read_vector(file, &format!("{prefix}.weight")).expect("LN weight");
            let bias = safetensors::read_vector(file, &format!("{prefix}.bias")).expect("LN bias");
            NormKind::new(activation, weight, bias, eps)
        };
        let wrap = |linear: F32Linear| {
            if quantize {
                Linear::Quant(QuantLinear::from(&linear))
            } else {
                Linear::F32(linear)
            }
        };
        let mut built = Vec::with_capacity(layers);
        for index in 0..layers {
            let prefix = format!("encoder.layer.{index}");
            built.push(BertLayer {
                query: wrap(tensor(&format!("{prefix}.attention.self.query.weight"))),
                key: wrap(tensor(&format!("{prefix}.attention.self.key.weight"))),
                value: wrap(tensor(&format!("{prefix}.attention.self.value.weight"))),
                output: wrap(tensor(&format!("{prefix}.attention.output.dense.weight"))),
                norm1: norm(&format!("{prefix}.attention.output.LayerNorm")),
                intermediate: wrap(tensor(&format!("{prefix}.intermediate.dense.weight"))),
                ffn_output: wrap(tensor(&format!("{prefix}.output.dense.weight"))),
                norm2: norm(&format!("{prefix}.output.LayerNorm")),
                activation,
            });
        }
        let _ = intermediate;
        Self {
            hidden,
            heads,
            head_dim: hidden / heads,
            layers: built,
            scales: None,
        }
    }

    pub(crate) fn set_scales(&mut self, scales: Vec<LayerScales>) {
        self.scales = Some(scales);
    }

    /// 前向；`capture` 为真时返回每层 FFN 输出段（GELU 后）的输入快照。
    pub(crate) fn forward(
        &self,
        input: &[f32],
        mask: &[u8],
        batch: usize,
        seq: usize,
        capture: bool,
    ) -> (Vec<f32>, Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let mut output = Vec::with_capacity(batch * seq * self.hidden);
        let mut snapshots = if capture {
            vec![Vec::new(); self.layers.len()]
        } else {
            Vec::new()
        };
        let mut layer_outputs = vec![Vec::new(); self.layers.len()];
        for item in 0..batch {
            let base = item * seq * self.hidden;
            let mut current = input[base..base + seq * self.hidden].to_vec();
            let item_mask = &mask[item * seq..(item + 1) * seq];
            for (index, layer) in self.layers.iter().enumerate() {
                let layer_scales = self.scales.as_ref().map(|all| &all[index]);
                let captured = layer.forward(
                    &mut current,
                    item_mask,
                    seq,
                    self.hidden,
                    self.heads,
                    self.head_dim,
                    layer_scales,
                );
                if capture {
                    snapshots[index].extend_from_slice(&captured);
                }
                layer_outputs[index].extend_from_slice(&current);
            }
            output.extend_from_slice(&current);
        }
        (output, snapshots, layer_outputs)
    }
}

impl BertLayer {
    #[allow(clippy::too_many_arguments)]
    fn forward(
        &self,
        x: &mut [f32],
        mask: &[u8],
        seq: usize,
        hidden: usize,
        heads: usize,
        head_dim: usize,
        scales: Option<&LayerScales>,
    ) -> Vec<f32> {
        let mut q = vec![0.0f32; seq * hidden];
        let mut k = vec![0.0f32; seq * hidden];
        let mut v = vec![0.0f32; seq * hidden];
        self.query.forward_seq(x, seq, &mut q);
        self.key.forward_seq(x, seq, &mut k);
        self.value.forward_seq(x, seq, &mut v);

        let scale = 1.0 / (head_dim as f32).sqrt();
        let mut context = vec![0.0f32; seq * hidden];
        let mut scores = vec![0.0f32; seq];
        for head in 0..heads {
            let offset = head * head_dim;
            for row in 0..seq {
                for col in 0..seq {
                    if mask[col] == 0 {
                        scores[col] = f32::NEG_INFINITY;
                    } else {
                        let mut dot = 0.0f32;
                        for d in 0..head_dim {
                            dot += q[row * hidden + offset + d] * k[col * hidden + offset + d];
                        }
                        scores[col] = dot * scale;
                    }
                }
                softmax(&mut scores);
                for d in 0..head_dim {
                    let mut sum = 0.0f32;
                    for col in 0..seq {
                        sum += scores[col] * v[col * hidden + offset + d];
                    }
                    context[row * hidden + offset + d] = sum;
                }
            }
        }

        let mut projected = vec![0.0f32; seq * hidden];
        self.output.forward_seq(&context, seq, &mut projected);
        for index in 0..seq * hidden {
            x[index] += projected[index];
        }
        self.norm1
            .forward_seq(x, seq, self.activation, scales.map(|s| s.norm1.as_slice()));

        let intermediate = hidden * 4;
        let mut ffn = vec![0.0f32; seq * intermediate];
        self.intermediate.forward_seq(x, seq, &mut ffn);
        for value in &mut ffn {
            *value = gelu(*value);
        }
        if self.activation.ternary_activation() {
            quantize_activation(&mut ffn, self.activation, scales.map(|s| s.ffn.as_slice()));
        }
        let snapshot = ffn.clone();
        let mut out = vec![0.0f32; seq * hidden];
        self.ffn_output.forward_seq(&ffn, seq, &mut out);
        for index in 0..seq * hidden {
            x[index] += out[index];
        }
        self.norm2
            .forward_seq(x, seq, self.activation, scales.map(|s| s.norm2.as_slice()));
        snapshot
    }
}

fn softmax(values: &mut [f32]) {
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for value in values.iter_mut() {
        *value = (*value - max).exp();
        sum += *value;
    }
    if sum > 0.0 {
        for value in values.iter_mut() {
            *value /= sum;
        }
    }
}

pub(crate) fn gelu(value: f32) -> f32 {
    let x = f64::from(value);
    (0.5 * x * (1.0 + erf(x / std::f64::consts::SQRT_2))) as f32
}

fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}
