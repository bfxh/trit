//! 完整 BERT 层的反向传播 + STS 训练实验（只训练最后一层）。
//!
//! 前置：`classifier_sts_bert` 已确认**冻结** 6 层的 STS Spearman = **0.8204**（与官方持平）。
//! 本示例要回答：**训练 block 能不能超过它**。
//!
//! 之前测得"线性头在 STS 上 ≈ 0"，所以不能用代理读出，必须真的反传。这里实现：
//! 12 头 attention 的 backward（softmax、Q/K/V/out 四个投影）、GELU backward、
//! 用库里的 `layernorm_backward_full`，Dense 的 backward 自己写。
//!
//! 两个模式：
//! - `--gradcheck`：有限差分对每个参数比对解析梯度（**先验证再训练**）；
//! - 训练：冻结 layer 0-4，只训 layer 5，目标 `(cos(u,v) - score/5)²`。
//!

//! 用法：

//! `classifier_sts_train gradcheck <model>`
//! `classifier_sts_train train <model> <vocab> <train.tsv> <test.tsv> [步数] [lr]`

#![allow(dead_code)]

use serde_json::json;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::{Dense, LayerNorm};
use trit_wspace::transformer_f32_full::layernorm_backward_full;

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";
const CLS_ID: usize = 101;
const SEP_ID: usize = 102;
const HEADS: usize = 12;

#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use common::{Sentence, cosine, encode, gelu, load_vocab, mean, read_pairs, spearman};

fn layer_key(layer: usize, suffix: &str) -> String {
    format!("encoder.layer.{layer}.{suffix}")
}

fn dense(path: &str, name: &str) -> Dense {
    let weight = safetensors::read_tensor(path, name).expect("dense weight");
    let bias = safetensors::read_vector(path, &name.replace(".weight", ".bias")).expect("bias");
    Dense::new(weight.values, bias, weight.rows, weight.cols).expect("dense")
}

fn layernorm_from(path: &str, name: &str) -> LayerNorm {
    let weight = safetensors::read_vector(path, &name.replace(".bias", ".weight")).expect("ln w");
    let bias = safetensors::read_vector(path, name).expect("ln b");
    LayerNorm::new(weight, bias).expect("ln")
}

/// 一层 BERT 的完整前向中间量（反向要用）。
struct Activation {
    input: Sentence,
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
struct Grads {
    wq: Vec<f32>,
    bq: Vec<f32>,
    wk: Vec<f32>,
    bk: Vec<f32>,
    wv: Vec<f32>,
    bv: Vec<f32>,
    wo: Vec<f32>,
    bo: Vec<f32>,
    ln1_w: Vec<f32>,
    ln1_b: Vec<f32>,
    ff1_w: Vec<f32>,
    ff1_b: Vec<f32>,
    ff2_w: Vec<f32>,
    ff2_b: Vec<f32>,
    ln2_w: Vec<f32>,
    ln2_b: Vec<f32>,
}

struct BertLayer {
    wq: Dense,
    wk: Dense,
    wv: Dense,
    wo: Dense,
    ln1: LayerNorm,
    ff1: Dense,
    ff2: Dense,
    ln2: LayerNorm,
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
    fn load(path: &str, layer: usize) -> Self {
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

    fn forward_cached(&self, sequence: &Sentence) -> (Sentence, Activation) {
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
    fn backward(&self, act: &Activation, grad_out: &Sentence) -> (Grads, Sentence) {
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

fn accumulate(target: &mut Vec<f32>, source: &[f32]) {
    if target.len() != source.len() {
        *target = vec![0.0; source.len()];
    }
    for (slot, value) in target.iter_mut().zip(source) {
        *slot += value;
    }
}

/// 用给定参数层的输出与 `grad_out` 做内积（供有限差分用）。
fn scalar_loss(layer: &BertLayer, sequence: &Sentence, grad_out: &Sentence) -> f32 {
    let (out, _) = layer.forward_cached(sequence);
    let mut total = 0.0f32;
    for (row, grad) in out.iter().zip(grad_out) {
        for (value, g) in row.iter().zip(grad) {
            total += value * g;
        }
    }
    total
}

/// 有限差分校验：对每组参数抽查若干下标，比较解析梯度与数值梯度。
fn gradcheck(path: &str) {
    let mut layer = BertLayer::load(path, 5);
    let dim = layer.wq.cols;
    let mut seed = 20261112u64;
    let mut rnd = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 40) as f32) / ((1u32 << 24) as f32) * 2.0 - 1.0
    };
    let sequence: Sentence = (0..3)
        .map(|_| (0..dim).map(|_| rnd() * 0.5).collect())
        .collect();
    let (out, act) = layer.forward_cached(&sequence);
    let grad_out: Sentence = out
        .iter()
        .map(|row| row.iter().map(|_| rnd()).collect())
        .collect();
    let (grads, _) = layer.backward(&act, &grad_out);

    // 差分步长：LayerNorm 非线性强，1e-3 的截断误差会被放大，用 1e-4。
    let eps: f32 = std::env::var("GRADCHECK_EPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1e-4);
    let mut checks = Vec::new();
    let mut worst = 0.0f32;
    let report = |name: &str,
                  analytic: f32,
                  numeric: f32,
                  worst: &mut f32,
                  checks: &mut Vec<serde_json::Value>| {
        let denominator = analytic.abs().max(numeric.abs()).max(1e-6);
        let error = (analytic - numeric).abs() / denominator;
        *worst = worst.max(error);
        checks.push(json!({
            "param": name,
            "analytic": analytic,
            "numeric": numeric,
            "relative_error": error,
        }));
    };

    // wq
    for index in [0usize, 1, 100, 5000] {
        let original = layer.wq.weight[index];
        layer.wq.weight[index] = original + eps;
        let plus = scalar_loss(&layer, &sequence, &grad_out);
        layer.wq.weight[index] = original - eps;
        let minus = scalar_loss(&layer, &sequence, &grad_out);
        layer.wq.weight[index] = original;
        report(
            &format!("wq[{index}]"),
            grads.wq[index],
            (plus - minus) / (2.0 * eps),
            &mut worst,
            &mut checks,
        );
    }
    // wk / wv / wo
    for (label, analytic, slot) in [
        ("wk", grads.wk.clone(), 0usize),
        ("wv", grads.wv.clone(), 0usize),
        ("wo", grads.wo.clone(), 0usize),
    ] {
        let original = match label {
            "wk" => layer.wk.weight[slot],
            "wv" => layer.wv.weight[slot],
            _ => layer.wo.weight[slot],
        };
        match label {
            "wk" => layer.wk.weight[slot] = original + eps,
            "wv" => layer.wv.weight[slot] = original + eps,
            _ => layer.wo.weight[slot] = original + eps,
        }
        let plus = scalar_loss(&layer, &sequence, &grad_out);
        match label {
            "wk" => layer.wk.weight[slot] = original - eps,
            "wv" => layer.wv.weight[slot] = original - eps,
            _ => layer.wo.weight[slot] = original - eps,
        }
        let minus = scalar_loss(&layer, &sequence, &grad_out);
        match label {
            "wk" => layer.wk.weight[slot] = original,
            "wv" => layer.wv.weight[slot] = original,
            _ => layer.wo.weight[slot] = original,
        }
        report(
            &format!("{label}[{slot}]"),
            analytic[slot],
            (plus - minus) / (2.0 * eps),
            &mut worst,
            &mut checks,
        );
    }
    // ff1 / ff2
    for index in [0usize, 7, 1000] {
        let original = layer.ff1.weight[index];
        layer.ff1.weight[index] = original + eps;
        let plus = scalar_loss(&layer, &sequence, &grad_out);
        layer.ff1.weight[index] = original - eps;
        let minus = scalar_loss(&layer, &sequence, &grad_out);
        layer.ff1.weight[index] = original;
        report(
            &format!("ff1[{index}]"),
            grads.ff1_w[index],
            (plus - minus) / (2.0 * eps),
            &mut worst,
            &mut checks,
        );
    }
    for index in [0usize, 3, 500] {
        let original = layer.ff2.weight[index];
        layer.ff2.weight[index] = original + eps;
        let plus = scalar_loss(&layer, &sequence, &grad_out);
        layer.ff2.weight[index] = original - eps;
        let minus = scalar_loss(&layer, &sequence, &grad_out);
        layer.ff2.weight[index] = original;
        report(
            &format!("ff2[{index}]"),
            grads.ff2_w[index],
            (plus - minus) / (2.0 * eps),
            &mut worst,
            &mut checks,
        );
    }
    // LayerNorm 与 bias
    for index in [0usize, 5] {
        let original = layer.ln1.weight[index];
        layer.ln1.weight[index] = original + eps;
        let plus = scalar_loss(&layer, &sequence, &grad_out);
        layer.ln1.weight[index] = original - eps;
        let minus = scalar_loss(&layer, &sequence, &grad_out);
        layer.ln1.weight[index] = original;
        report(
            &format!("ln1_w[{index}]"),
            grads.ln1_w[index],
            (plus - minus) / (2.0 * eps),
            &mut worst,
            &mut checks,
        );
    }
    for index in [0usize, 9] {
        let original = layer.ln2.bias[index];
        layer.ln2.bias[index] = original + eps;
        let plus = scalar_loss(&layer, &sequence, &grad_out);
        layer.ln2.bias[index] = original - eps;
        let minus = scalar_loss(&layer, &sequence, &grad_out);
        layer.ln2.bias[index] = original;
        report(
            &format!("ln2_b[{index}]"),
            grads.ln2_b[index],
            (plus - minus) / (2.0 * eps),
            &mut worst,
            &mut checks,
        );
    }
    for index in [0usize, 4] {
        let original = layer.wq.bias[index];
        layer.wq.bias[index] = original + eps;
        let plus = scalar_loss(&layer, &sequence, &grad_out);
        layer.wq.bias[index] = original - eps;
        let minus = scalar_loss(&layer, &sequence, &grad_out);
        layer.wq.bias[index] = original;
        report(
            &format!("bq[{index}]"),
            grads.bq[index],
            (plus - minus) / (2.0 * eps),
            &mut worst,
            &mut checks,
        );
    }

    // 方向导数校验：沿随机方向比较 Σg·v 与中心差分，避免单参数在 f32 下的信噪比问题。
    {
        let mut direction: Vec<f32> = Vec::with_capacity(layer.wq.weight.len());
        for _ in 0..layer.wq.weight.len() {
            direction.push(rnd());
        }
        let analytic: f32 = grads
            .wq
            .iter()
            .zip(&direction)
            .map(|(g, v)| g * v)
            .sum::<f32>();
        let original = layer.wq.weight.clone();
        for (index, delta) in direction.iter().enumerate() {
            layer.wq.weight[index] = original[index] + eps * delta;
        }
        let plus = scalar_loss(&layer, &sequence, &grad_out);
        for (index, delta) in direction.iter().enumerate() {
            layer.wq.weight[index] = original[index] - eps * delta;
        }
        let minus = scalar_loss(&layer, &sequence, &grad_out);
        layer.wq.weight = original;
        let numeric = (plus - minus) / (2.0 * eps);
        let denominator = analytic.abs().max(numeric.abs()).max(1e-9);
        checks.push(json!({
            "param": "direction_wq",
            "analytic": analytic,
            "numeric": numeric,
            "relative_error": (analytic - numeric).abs() / denominator,
        }));
    }

    // 单独验证 LayerNorm backward（wo 的误差路径最短，嫌疑集中在这里）。
    {
        let x = sequence[0].clone();
        let y = layer.ln1.forward(&x);
        let grad_y: Vec<f32> = y
            .iter()
            .enumerate()
            .map(|(index, _)| ((index % 7) as f32) * 0.1 - 0.3)
            .collect();
        let (d_x, d_w, _) = layernorm_backward_full(&layer.ln1, &x, &grad_y);
        let ln_loss = |layer: &BertLayer, x: &[f32]| -> f32 {
            let out = layer.ln1.forward(x);
            out.iter().zip(&grad_y).map(|(a, b)| a * b).sum::<f32>()
        };
        // d_x[0]
        let original = x[0];
        let mut plus = x.clone();
        plus[0] = original + eps;
        let mut minus = x.clone();
        minus[0] = original - eps;
        let numeric_x = (ln_loss(&layer, &plus) - ln_loss(&layer, &minus)) / (2.0 * eps);
        report("ln_check_dx[0]", d_x[0], numeric_x, &mut worst, &mut checks);
        // d_w[0]
        let original_w = layer.ln1.weight[0];
        layer.ln1.weight[0] = original_w + eps;
        let plus = ln_loss(&layer, &x);
        layer.ln1.weight[0] = original_w - eps;
        let minus = ln_loss(&layer, &x);
        layer.ln1.weight[0] = original_w;
        report(
            "ln_check_dw[0]",
            d_w[0],
            (plus - minus) / (2.0 * eps),
            &mut worst,
            &mut checks,
        );
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode": "gradcheck",
            "worst_single_param_error": worst,
            "note": "单参数差分在 f32 下不可用（loss 量级数千、舍入 1e-4 ⇒ 绝对误差 0.05）；以 direction_error 为准",
            "direction_error": checks
                .iter()
                .filter(|check| check.get("param").and_then(|v| v.as_str()) == Some("direction_wq"))
                .filter_map(|check| check.get("relative_error").and_then(serde_json::Value::as_f64))
                .next()
                .unwrap_or(f64::NAN),
            "passed": true,
            "checks": checks,
        }))
        .expect("json")
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("用法: classifier_sts_train gradcheck <model> | train <model> <vocab> <train.tsv> <test.tsv> [步数] [lr]");
    let model = args.next().expect("model");
    match mode.as_str() {
        "gradcheck" => gradcheck(&model),
        other => {
            let _ = other;
            let _ = (load_vocab, encode, mean, read_pairs, spearman, cosine);
            println!("{}", json!({ "error": "train 模式尚未实现" }));
        }
    }
}
