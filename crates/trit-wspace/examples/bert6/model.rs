//! MiniLM BERT 6 层的 f32 / 三值前向（只三值化线性权重，LN/GELU/softmax 保持 f32）。

use trit_wspace::safetensors::{self, Weights};

pub(crate) struct F32Linear {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) weight: Vec<f32>,
    pub(crate) bias: Vec<f32>,
}

impl F32Linear {
    fn forward(&self, input: &[f32], out: &mut [f32]) {
        for (row, slot) in out.iter_mut().enumerate().take(self.rows) {
            let mut acc = f64::from(self.bias[row]);
            let line = &self.weight[row * self.cols..(row + 1) * self.cols];
            for (&weight, &value) in line.iter().zip(input) {
                acc += f64::from(weight) * f64::from(value);
            }
            *slot = acc as f32;
        }
    }

    fn forward_seq(&self, input: &[f32], seq: usize, out: &mut [f32]) {
        for token in 0..seq {
            self.forward(
                &input[token * self.cols..(token + 1) * self.cols],
                &mut out[token * self.rows..(token + 1) * self.rows],
            );
        }
    }
}

pub(crate) struct QuantLinear {
    rows: usize,
    cols: usize,
    trits: Vec<i8>,
    scales: Vec<f32>,
    bias: Vec<f32>,
}

impl QuantLinear {
    fn from(linear: &F32Linear) -> Self {
        let weights = Weights {
            rows: linear.rows,
            cols: linear.cols,
            values: linear.weight.clone(),
        };
        let layer = weights
            .to_layer_with_bias(Some(&linear.bias))
            .expect("三值化失败");
        let mut trits = Vec::with_capacity(linear.rows * linear.cols);
        let mut scales = Vec::with_capacity(linear.rows);
        for row in 0..linear.rows {
            scales.push(layer.scale(row).expect("scale"));
            for col in 0..linear.cols {
                trits.push(layer.trit(row, col).expect("trit").to_i8());
            }
        }
        Self {
            rows: linear.rows,
            cols: linear.cols,
            trits,
            scales,
            bias: linear.bias.clone(),
        }
    }

    fn forward(&self, input: &[f32], out: &mut [f32]) {
        for (row, slot) in out.iter_mut().enumerate().take(self.rows) {
            let mut acc = 0.0f64;
            let line = &self.trits[row * self.cols..(row + 1) * self.cols];
            for (&trit, &value) in line.iter().zip(input) {
                acc += f64::from(trit) * f64::from(value);
            }
            *slot = (f64::from(self.scales[row]) * acc + f64::from(self.bias[row])) as f32;
        }
    }

    fn forward_seq(&self, input: &[f32], seq: usize, out: &mut [f32]) {
        for token in 0..seq {
            self.forward(
                &input[token * self.cols..(token + 1) * self.cols],
                &mut out[token * self.rows..(token + 1) * self.rows],
            );
        }
    }
}

pub(crate) struct Norm {
    weight: Vec<f32>,
    bias: Vec<f32>,
    eps: f32,
}

impl Norm {
    fn forward(&self, values: &mut [f32]) {
        let n = values.len() as f32;
        let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / f64::from(n);
        let var = values
            .iter()
            .map(|&v| {
                let d = f64::from(v) - mean;
                d * d
            })
            .sum::<f64>()
            / f64::from(n);
        let inv = 1.0 / (var + f64::from(self.eps)).sqrt();
        for (index, value) in values.iter_mut().enumerate() {
            *value =
                (((f64::from(*value) - mean) * inv) as f32) * self.weight[index] + self.bias[index];
        }
    }

    fn forward_seq(&self, values: &mut [f32], seq: usize) {
        let width = self.weight.len();
        for token in 0..seq {
            self.forward(&mut values[token * width..(token + 1) * width]);
        }
    }
}

enum Linear {
    F32(F32Linear),
    Quant(QuantLinear),
}

impl Linear {
    fn forward_seq(&self, input: &[f32], seq: usize, out: &mut [f32]) {
        match self {
            Self::F32(linear) => linear.forward_seq(input, seq, out),
            Self::Quant(linear) => linear.forward_seq(input, seq, out),
        }
    }
}

pub(crate) struct BertLayer {
    query: Linear,
    key: Linear,
    value: Linear,
    output: Linear,
    norm1: Norm,
    intermediate: Linear,
    ffn_output: Linear,
    norm2: Norm,
}

pub(crate) struct Bert {
    hidden: usize,
    heads: usize,
    head_dim: usize,
    layers: Vec<BertLayer>,
}

impl Bert {
    pub(crate) fn load(
        file: &str,
        layers: usize,
        hidden: usize,
        intermediate: usize,
        heads: usize,
        eps: f32,
        quantize: bool,
    ) -> Self {
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
        let norm = |prefix: &str| -> Norm {
            let weight =
                safetensors::read_vector(file, &format!("{prefix}.weight")).expect("LN weight");
            let bias = safetensors::read_vector(file, &format!("{prefix}.bias")).expect("LN bias");
            Norm { weight, bias, eps }
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
            });
        }
        let _ = intermediate;
        Self {
            hidden,
            heads,
            head_dim: hidden / heads,
            layers: built,
        }
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
                let captured = layer.forward(
                    &mut current,
                    item_mask,
                    seq,
                    self.hidden,
                    self.heads,
                    self.head_dim,
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
    fn forward(
        &self,
        x: &mut [f32],
        mask: &[u8],
        seq: usize,
        hidden: usize,
        heads: usize,
        head_dim: usize,
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
        self.norm1.forward_seq(x, seq);

        let intermediate = hidden * 4;
        let mut ffn = vec![0.0f32; seq * intermediate];
        self.intermediate.forward_seq(x, seq, &mut ffn);
        for value in &mut ffn {
            *value = gelu(*value);
        }
        let snapshot = ffn.clone();
        let mut out = vec![0.0f32; seq * hidden];
        self.ffn_output.forward_seq(&ffn, seq, &mut out);
        for index in 0..seq * hidden {
            x[index] += out[index];
        }
        self.norm2.forward_seq(x, seq);
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
