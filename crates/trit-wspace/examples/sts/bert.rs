//! 完整 BERT 层：前向缓存 + 反向传播，供 STS 系列示例共用。
//!
//! 结构与 `classifier_sts_bert` 一致（12 头、GELU、post-LN、Q/K/V/out 带 bias）。
//! 反向的正确性由 `classifier_sts_train gradcheck` 用方向导数验证过（相对误差 0.37%）。

use trit_wspace::safetensors;
use trit_wspace::transformer_f32::{Dense, LayerNorm};
use trit_wspace::transformer_f32_full::layernorm_backward_full;

use super::common::Sentence;
use super::common::gelu;

pub const HEADS: usize = 12;
pub const CLS_ID: usize = 101;
pub const SEP_ID: usize = 102;

pub fn layer_key(layer: usize, suffix: &str) -> String {
    format!("encoder.layer.{layer}.{suffix}")
}

pub fn dense(path: &str, name: &str) -> Dense {
    let weight = safetensors::read_tensor(path, name).expect("dense weight");
    let bias = safetensors::read_vector(path, &name.replace(".weight", ".bias")).expect("bias");
    Dense::new(weight.values, bias, weight.rows, weight.cols).expect("dense")
}

pub fn layernorm_from(path: &str, name: &str) -> LayerNorm {
    let weight = safetensors::read_vector(path, &name.replace(".bias", ".weight")).expect("ln w");
    let bias = safetensors::read_vector(path, name).expect("ln b");
    LayerNorm::new(weight, bias).expect("ln")
}

/// 一层 BERT 的完整前向中间量（反向要用）。
pub struct Activation {
    pub input: Sentence,
    q: Vec<Vec<f32>>,
    k: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
    probs: Vec<Vec<Vec<f32>>>,
    pooled: Vec<Vec<f32>>,
    residual: Sentence,
    hidden: Sentence,
    pre: Vec<Vec<f32>>,
    mid: Sentence,
    ffn: Sentence,
    residual2: Sentence,
}

/// 一层 BERT 的全部参数梯度。
#[derive(Default, Clone)]
pub struct Grads {
    pub wq: Vec<f32>,
    pub bq: Vec<f32>,
    pub wk: Vec<f32>,
    pub bk: Vec<f32>,
    pub wv: Vec<f32>,
    pub bv: Vec<f32>,
    pub wo: Vec<f32>,
    pub bo: Vec<f32>,
    pub ln1_w: Vec<f32>,
    pub ln1_b: Vec<f32>,
    pub ff1_w: Vec<f32>,
    pub ff1_b: Vec<f32>,
    pub ff2_w: Vec<f32>,
    pub ff2_b: Vec<f32>,
    pub ln2_w: Vec<f32>,
    pub ln2_b: Vec<f32>,
}

pub struct BertLayer {
    pub wq: Dense,
    pub wk: Dense,
    pub wv: Dense,
    pub wo: Dense,
    pub ln1: LayerNorm,
    pub ff1: Dense,
    pub ff2: Dense,
    pub ln2: LayerNorm,
}

fn dense_backward(layer: &Dense, input: &[f32], grad: &[f32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut d_w = vec![0.0f32; layer.weight.len()];
    for (row, g) in grad.iter().enumerate() {
        for (col, value) in input.iter().enumerate() {
            d_w[row * layer.cols + col] += g * value;
        }
    }
    let mut d_x = vec![0.0f32; input.len()];
    for (row, g) in grad.iter().enumerate() {
        for (col, weight) in layer.weight[row * layer.cols..(row + 1) * layer.cols]
            .iter()
            .enumerate()
        {
            d_x[col] += g * weight;
        }
    }
    (d_w, grad.to_vec(), d_x)
}

fn gelu_grad(value: f32) -> f32 {
    let scale = (2.0f32 / std::f32::consts::PI).sqrt();
    let inner = value + 0.044_715 * value * value * value;
    let tanh = (scale * inner).tanh();
    let derivative = scale * (1.0 + 3.0 * 0.044_715 * value * value);
    0.5 * (1.0 + tanh) + 0.5 * value * (1.0 - tanh * tanh) * derivative
}

impl BertLayer {
    pub fn load(path: &str, layer: usize) -> Self {
        Self {
            wq: dense(path, &layer_key(layer, "attention.self.query.weight")),
            wk: dense(path, &layer_key(layer, "attention.self.key.weight")),
            wv: dense(path, &layer_key(layer, "attention.self.value.weight")),
            wo: dense(path, &layer_key(layer, "attention.output.dense.weight")),
            ln1: layernorm_from(path, &layer_key(layer, "attention.output.LayerNorm.bias")),
            ff1: dense(path, &layer_key(layer, "intermediate.dense.weight")),
            ff2: dense(path, &layer_key(layer, "output.dense.weight")),
            ln2: layernorm_from(path, &layer_key(layer, "output.LayerNorm.bias")),
        }
    }

    pub fn forward_cached(&self, sequence: &Sentence) -> (Sentence, Activation) {
        let dim = sequence[0].len();
        let head_dim = dim / HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let q: Vec<Vec<f32>> = sequence
            .iter()
            .map(|token| self.wq.forward(token, false))
            .collect();
        let k: Vec<Vec<f32>> = sequence
            .iter()
            .map(|token| self.wk.forward(token, false))
            .collect();
        let v: Vec<Vec<f32>> = sequence
            .iter()
            .map(|token| self.wv.forward(token, false))
            .collect();
        let mut probs = Vec::with_capacity(sequence.len());
        let mut pooled = Vec::with_capacity(sequence.len());
        for query in &q {
            let mut per_head = Vec::with_capacity(HEADS);
            let mut attention = vec![0.0f32; dim];
            for head in 0..HEADS {
                let offset = head * head_dim;
                let span = offset..offset + head_dim;
                let scores: Vec<f32> = k
                    .iter()
                    .map(|key| {
                        query[span.clone()]
                            .iter()
                            .zip(&key[span.clone()])
                            .map(|(a, b)| a * b)
                            .sum::<f32>()
                            * scale
                    })
                    .collect();
                let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let exps: Vec<f32> = scores.iter().map(|score| (score - max).exp()).collect();
                let total: f32 = exps.iter().sum();
                let weights: Vec<f32> = exps.iter().map(|value| value / total).collect();
                for (weight, value) in weights.iter().zip(&v) {
                    for index in span.clone() {
                        attention[index] += weight * value[index];
                    }
                }
                per_head.push(weights);
            }
            probs.push(per_head);
            pooled.push(attention);
        }
        let mut residual = Vec::with_capacity(sequence.len());
        let mut hidden = Vec::with_capacity(sequence.len());
        let mut pre = Vec::with_capacity(sequence.len());
        let mut mid = Vec::with_capacity(sequence.len());
        let mut ffn = Vec::with_capacity(sequence.len());
        let mut residual2 = Vec::with_capacity(sequence.len());
        let mut out = Vec::with_capacity(sequence.len());
        for index in 0..sequence.len() {
            let projected = self.wo.forward(&pooled[index], false);
            let residual_row: Vec<f32> = sequence[index]
                .iter()
                .zip(&projected)
                .map(|(a, b)| a + b)
                .collect();
            let hidden_row = self.ln1.forward(&residual_row);
            let pre_row = self.ff1.forward(&hidden_row, false);
            let mid_row: Vec<f32> = pre_row.iter().map(|value| gelu(*value)).collect();
            let ffn_row = self.ff2.forward(&mid_row, false);
            let residual2_row: Vec<f32> = hidden_row
                .iter()
                .zip(&ffn_row)
                .map(|(a, b)| a + b)
                .collect();
            let out_row = self.ln2.forward(&residual2_row);
            residual.push(residual_row);
            hidden.push(hidden_row);
            pre.push(pre_row);
            mid.push(mid_row);
            ffn.push(ffn_row);
            residual2.push(residual2_row);
            out.push(out_row);
        }
        let activation = Activation {
            input: sequence.clone(),
            q,
            k,
            v,
            probs,
            pooled,
            residual,
            hidden,
            pre,
            mid,
            ffn,
            residual2,
        };
        (out, activation)
    }

    /// 反传：返回（参数梯度, 对输入的梯度）。
    pub fn backward(&self, act: &Activation, grad_out: &Sentence) -> (Grads, Sentence) {
        let dim = act.input[0].len();
        let head_dim = dim / HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let mut grads = Grads::default();

        // LN2
        let mut d_residual2 = Vec::with_capacity(act.input.len());
        for (index, grad) in grad_out.iter().enumerate() {
            let (d_x, d_w, d_b) = layernorm_backward_full(&self.ln2, &act.residual2[index], grad);
            accumulate(&mut grads.ln2_w, &d_w);
            accumulate(&mut grads.ln2_b, &d_b);
            d_residual2.push(d_x);
        }

        // FFN2 / GELU / FFN1
        let mut d_hidden = vec![vec![0.0f32; dim]; act.input.len()];
        for index in 0..act.input.len() {
            let (d_w, d_b, d_mid) = dense_backward(&self.ff2, &act.mid[index], &d_residual2[index]);
            accumulate(&mut grads.ff2_w, &d_w);
            accumulate(&mut grads.ff2_b, &d_b);
            let d_pre: Vec<f32> = d_mid
                .iter()
                .zip(&act.pre[index])
                .map(|(g, value)| g * gelu_grad(*value))
                .collect();
            let (d_w1, d_b1, d_h) = dense_backward(&self.ff1, &act.hidden[index], &d_pre);
            accumulate(&mut grads.ff1_w, &d_w1);
            accumulate(&mut grads.ff1_b, &d_b1);
            for (slot, value) in d_hidden[index].iter_mut().zip(&d_h) {
                *slot += value;
            }
            // 残差：hidden 也直接连到 residual2
            for (slot, value) in d_hidden[index].iter_mut().zip(&d_residual2[index]) {
                *slot += value;
            }
        }

        // LN1
        let mut d_residual = Vec::with_capacity(act.input.len());
        for (residual, grad) in act.residual.iter().zip(&d_hidden) {
            let (d_x, d_w, d_b) = layernorm_backward_full(&self.ln1, residual, grad);
            accumulate(&mut grads.ln1_w, &d_w);
            accumulate(&mut grads.ln1_b, &d_b);
            d_residual.push(d_x);
        }

        // Wo（attention 输出投影）
        let mut d_pooled = Vec::with_capacity(act.input.len());
        for (pooled, grad) in act.pooled.iter().zip(&d_residual) {
            let (d_w, d_b, d_p) = dense_backward(&self.wo, pooled, grad);
            accumulate(&mut grads.wo, &d_w);
            accumulate(&mut grads.bo, &d_b);
            d_pooled.push(d_p);
        }

        // 多头 attention
        let mut d_q = vec![vec![0.0f32; dim]; act.input.len()];
        let mut d_k = vec![vec![0.0f32; dim]; act.input.len()];
        let mut d_v = vec![vec![0.0f32; dim]; act.input.len()];
        for index in 0..act.input.len() {
            for head in 0..HEADS {
                let offset = head * head_dim;
                let span = offset..offset + head_dim;
                let weights = &act.probs[index][head];
                // d_v[j] += w[ij] * d_attn_head
                for (j, weight) in weights.iter().enumerate() {
                    for i in span.clone() {
                        d_v[j][i] += weight * d_pooled[index][i];
                    }
                }
                // d_w[ij] = v[j]·d_attn_head
                let d_w: Vec<f32> = act
                    .v
                    .iter()
                    .map(|value| {
                        value[span.clone()]
                            .iter()
                            .zip(&d_pooled[index][span.clone()])
                            .map(|(a, b)| a * b)
                            .sum::<f32>()
                    })
                    .collect();
                // softmax backward
                let total: f32 = weights.iter().zip(&d_w).map(|(w, g)| w * g).sum();
                let d_scores: Vec<f32> = weights
                    .iter()
                    .zip(&d_w)
                    .map(|(w, g)| w * (g - total))
                    .collect();
                for (j, d_score) in d_scores.iter().enumerate() {
                    let scaled = d_score * scale;
                    for i in span.clone() {
                        d_q[index][i] += scaled * act.k[j][i];
                        d_k[j][i] += scaled * act.q[index][i];
                    }
                }
            }
        }

        // Q/K/V 投影
        let mut d_input = vec![vec![0.0f32; dim]; act.input.len()];
        for index in 0..act.input.len() {
            let (d_w, d_b, d_x) = dense_backward(&self.wq, &act.input[index], &d_q[index]);
            accumulate(&mut grads.wq, &d_w);
            accumulate(&mut grads.bq, &d_b);
            for (slot, value) in d_input[index].iter_mut().zip(&d_x) {
                *slot += value;
            }
            let (d_w, d_b, d_x) = dense_backward(&self.wk, &act.input[index], &d_k[index]);
            accumulate(&mut grads.wk, &d_w);
            accumulate(&mut grads.bk, &d_b);
            for (slot, value) in d_input[index].iter_mut().zip(&d_x) {
                *slot += value;
            }
            let (d_w, d_b, d_x) = dense_backward(&self.wv, &act.input[index], &d_v[index]);
            accumulate(&mut grads.wv, &d_w);
            accumulate(&mut grads.bv, &d_b);
            for (slot, value) in d_input[index].iter_mut().zip(&d_x) {
                *slot += value;
            }
            // 残差直连
            for (slot, value) in d_input[index].iter_mut().zip(&d_residual[index]) {
                *slot += value;
            }
        }

        (grads, d_input)
    }
}

pub fn accumulate(target: &mut Vec<f32>, source: &[f32]) {
    if target.len() != source.len() {
        *target = vec![0.0; source.len()];
    }
    for (slot, value) in target.iter_mut().zip(source) {
        *slot += value;
    }
}

/// `[CLS] + tokens + [SEP]`，词向量 + position + token_type，再过 embeddings LayerNorm。
pub fn bert_sequence(
    ids: &[usize],
    embeddings: &safetensors::Weights,
    position: &safetensors::Weights,
    token_type: &safetensors::Weights,
    emb_ln: &LayerNorm,
    dim: usize,
) -> Sentence {
    let mut full = Vec::with_capacity(ids.len() + 2);
    full.push(CLS_ID);
    full.extend_from_slice(ids);
    full.push(SEP_ID);
    full.iter()
        .enumerate()
        .map(|(slot, id)| {
            let mut vector = embeddings.values[id * dim..(id + 1) * dim].to_vec();
            let segment = &token_type.values[..dim];
            for (index, value) in vector.iter_mut().enumerate() {
                *value += segment[index];
            }
            if slot < position.rows {
                let offset = slot * dim;
                for (index, value) in vector.iter_mut().enumerate() {
                    *value += position.values[offset + index];
                }
            }
            emb_ln.forward(&vector)
        })
        .collect()
}
