//! 可解码性是否随深度出现？——MiniLM 6 层真实权重的逐层线性探针。
//!
//! 之前所有结论都基于单层 layer-0。本示例把 MiniLM-L6 的 6 层**真实权重**依次堆叠，
//! 对每层输出做闭式岭回归（留出集），看"表达/可解码性"是否随深度上升。
//!
//! 结构简化（有意为之，需在结论里声明）：
//! - 每个样本只对固定的 64 个 context token 做注意力（不做全序列 self-attention），
//!   否则 1024 个样本就是 1024² 的注意力；
//! - `F32Attention` 只吃 Q/K/V 权重，**忽略** safetensors 里的 attention bias；
//! - 没有 position embedding。
//!
//! 每层的 context 也用同一层参数前向，保证层间自洽。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::classifier_head::ClassifierHead;
use trit_wspace::safetensors;
use trit_wspace::teacher_f32::{Lcg, quad_labels, rank_labels};
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_cls::forward_with;

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const SAMPLES: usize = 1024;
const CONTEXT: usize = 64;
const CLASSES: usize = 4;
const LAYERS: usize = 6;
const TRAIN_RATIO: f32 = 0.75;
const LAMBDA: f32 = 1e-2;
const TEMPERATURE: f32 = 0.1;
const SEED: u64 = 20261112;

fn layer_key(layer: usize, suffix: &str) -> String {
    format!("encoder.layer.{layer}.{suffix}")
}

fn load_layer(path: &str, layer: usize, dim: usize) -> TransformerBlock {
    let query = safetensors::read_tensor(path, &layer_key(layer, "attention.self.query.weight"))
        .expect("query");
    let key = safetensors::read_tensor(path, &layer_key(layer, "attention.self.key.weight"))
        .expect("key");
    let value = safetensors::read_tensor(path, &layer_key(layer, "attention.self.value.weight"))
        .expect("value");
    let ln1_w =
        safetensors::read_vector(path, &layer_key(layer, "attention.output.LayerNorm.weight"))
            .expect("ln1 weight");
    let ln1_b =
        safetensors::read_vector(path, &layer_key(layer, "attention.output.LayerNorm.bias"))
            .expect("ln1 bias");
    let ff1_w = safetensors::read_tensor(path, &layer_key(layer, "intermediate.dense.weight"))
        .expect("ff1");
    let ff1_b = safetensors::read_vector(path, &layer_key(layer, "intermediate.dense.bias"))
        .expect("ff1 bias");
    let ff2_w =
        safetensors::read_tensor(path, &layer_key(layer, "output.dense.weight")).expect("ff2");
    let ff2_b =
        safetensors::read_vector(path, &layer_key(layer, "output.dense.bias")).expect("ff2 bias");
    let ln2_w = safetensors::read_vector(path, &layer_key(layer, "output.LayerNorm.weight"))
        .expect("ln2 weight");
    let ln2_b = safetensors::read_vector(path, &layer_key(layer, "output.LayerNorm.bias"))
        .expect("ln2 bias");
    TransformerBlock::new(
        F32Attention::new(query.values, key.values, value.values, dim, dim).expect("attention"),
        LayerNorm::new(ln1_w, ln1_b).expect("ln1"),
        Dense::new(ff1_w.values, ff1_b, ff1_w.rows, ff1_w.cols).expect("ff1"),
        Dense::new(ff2_w.values, ff2_b, ff2_w.rows, ff2_w.cols).expect("ff2"),
        LayerNorm::new(ln2_w, ln2_b).expect("ln2"),
    )
    .expect("block")
}

fn pick(features: &[Vec<f32>], indices: &[usize]) -> Vec<Vec<f32>> {
    indices
        .iter()
        .map(|index| features[*index].clone())
        .collect()
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm * right_norm)
    }
}

/// 上下文任务：`h = Σ softmax(cos/T)·ctx` 的固定投影（必须读 context）。
fn context_scores(samples: &[Vec<f32>], context: &[Vec<f32>], axis: &[f32]) -> Vec<f32> {
    samples
        .iter()
        .map(|sample| {
            let sims: Vec<f32> = context
                .iter()
                .map(|token| cosine(sample, token) / TEMPERATURE)
                .collect();
            let max = sims.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let exps: Vec<f32> = sims.iter().map(|value| (value - max).exp()).collect();
            let total: f32 = exps.iter().sum();
            let mut pooled = vec![0.0f32; axis.len()];
            for (weight, token) in exps.iter().zip(context) {
                let normalized = weight / total;
                for (slot, value) in pooled.iter_mut().zip(token) {
                    *slot += normalized * value;
                }
            }
            pooled
                .iter()
                .zip(axis)
                .map(|(value, weight)| value * weight)
                .sum()
        })
        .collect()
}

fn probe(
    features: &[Vec<f32>],
    labels: &[usize],
    train: &[usize],
    test: &[usize],
) -> serde_json::Value {
    let train_x = pick(features, train);
    let train_y: Vec<usize> = train.iter().map(|index| labels[*index]).collect();
    let test_x = pick(features, test);
    let test_y: Vec<usize> = test.iter().map(|index| labels[*index]).collect();
    match ClassifierHead::fit_ridge(&train_x, &train_y, LAMBDA) {
        Some(head) => json!({
            "train": head.accuracy(&train_x, &train_y).expect("train accuracy"),
            "test": head.accuracy(&test_x, &test_y).expect("test accuracy"),
        }),
        None => json!({ "error": "ridge degenerate" }),
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: classifier_layers <model.safetensors>");

    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = embeddings.cols;

    let samples: Vec<Vec<f32>> = (0..SAMPLES)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();
    let mut context: Vec<Vec<f32>> = (SAMPLES..SAMPLES + CONTEXT)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();

    let mut rng = Lcg::new(SEED);
    // 任务 A：只看自身（与之前单层 quad 可比）。
    let quad = quad_labels(&mut rng, &samples, CLASSES);
    // 任务 B：必须读上下文。
    let axis: Vec<f32> = (0..dim).map(|_| rng.next_f32() * 0.1).collect();
    let context_task = rank_labels(&context_scores(&samples, &context, &axis), CLASSES);

    let mut order: Vec<usize> = (0..SAMPLES).collect();
    for index in (1..SAMPLES).rev() {
        let draw = rng.next_f32().abs();
        let swap = (draw * (index + 1) as f32) as usize % (index + 1);
        order.swap(index, swap);
    }
    let split = (SAMPLES as f32 * TRAIN_RATIO) as usize;
    let (train, test) = order.split_at(split);

    let mut per_layer = Vec::with_capacity(LAYERS + 1);
    per_layer.push(json!({
        "layer": "raw",
        "quad": probe(&samples, &quad, train, test),
        "context": probe(&samples, &context_task, train, test),
    }));

    let mut current = samples.clone();
    for layer in 0..LAYERS {
        let block = load_layer(&path, layer, dim);
        let next_samples = forward_with(&block, &current, &context).expect("samples forward");
        let next_context = forward_with(&block, &context, &context).expect("context forward");
        current = next_samples;
        context = next_context;
        per_layer.push(json!({
            "layer": layer,
            "quad": probe(&current, &quad, train, test),
            "context": probe(&current, &context_task, train, test),
        }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "samples": SAMPLES,
            "context": CONTEXT,
            "layers": LAYERS,
            "train_samples": train.len(),
            "test_samples": test.len(),
            "classes": CLASSES,
            "lambda": LAMBDA,
            "chance": 1.0 / CLASSES as f32,
            "caveats": "samples attend only to a fixed 64-token context; attention bias and position embeddings ignored",
            "per_layer": per_layer,
            "finite": current.iter().flatten().all(|value| value.is_finite()),
        }))
        .expect("json")
    );
}
