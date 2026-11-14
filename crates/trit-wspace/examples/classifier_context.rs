//! 给块一个公平的判据：标签**必须读上下文**。
//!
//! 之前所有任务（`quad` / `linear`）的标签只由 token 自身决定，而块相对原始向量的唯一
//! 新增能力是"attention 读上下文 + FFN"——标签与上下文无关时，绕过块本就最优，
//! 0.3438 < 0.3945 是结构必然。
//!
//! 本示例换任务：
//! 1. 64 个 context token 各分一类（二次型得分按分位切 4 档，每类 16 个）；
//! 2. **样本标签 = 与样本最相似的那个 context token 的类别**。
//!
//! 这个标签只依赖"样本与上下文的相对结构"：只看样本自己的向量无法确定类别，
//! 必须读上下文。于是三方对比才有意义：
//!
//! - `raw`：原始词向量——预期接近机会线（它看不到 context 分类）；
//! - `block`：冻结块输出（attention 已经把 context 聚合进来）；
//! - `concat`：两者拼接。
//!
//! 三者都用闭式岭回归，不涉及训练循环。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::classifier_head::ClassifierHead;
use trit_wspace::safetensors;
use trit_wspace::teacher_f32::{Lcg, rank_labels};
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_cls::forward_with;

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const LN1_W: &str = "encoder.layer.0.attention.output.LayerNorm.weight";
const LN1_B: &str = "encoder.layer.0.attention.output.LayerNorm.bias";
const FF1_W: &str = "encoder.layer.0.intermediate.dense.weight";
const FF1_B: &str = "encoder.layer.0.intermediate.dense.bias";
const FF2_W: &str = "encoder.layer.0.output.dense.weight";
const FF2_B: &str = "encoder.layer.0.output.dense.bias";
const LN2_W: &str = "encoder.layer.0.output.LayerNorm.weight";
const LN2_B: &str = "encoder.layer.0.output.LayerNorm.bias";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const SAMPLES: usize = 1024;
const CONTEXT: usize = 64;
const CLASSES: usize = 4;
const TRAIN_RATIO: f32 = 0.75;
const LAMBDA: f32 = 1e-2;
const SEED: u64 = 20261112;
const TEMPERATURE: f32 = 0.1;

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

/// 上下文派生的连续得分：`h = Σ softmax(cos/T) · ctx`，再与固定方向 `axis` 取内积。
///
/// 用**软加权**而不是硬 argmax：硬取最近邻会让标签塌成常数（实测 995/1024 落入一类，
/// 三路探针都靠猜多数类拿到 0.99）。软加权让得分随样本连续变化，按分位切档后类别均衡，
/// 同时仍然必须读 context 才能算出来。
fn context_scores(
    samples: &[Vec<f32>],
    context: &[Vec<f32>],
    axis: &[f32],
    temperature: f32,
) -> Vec<f32> {
    samples
        .iter()
        .map(|sample| {
            let sims: Vec<f32> = context
                .iter()
                .map(|token| cosine(sample, token) / temperature)
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
    name: &str,
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
            "probe": name,
            "dim": features.first().map_or(0, Vec::len),
            "train": head.accuracy(&train_x, &train_y).expect("train accuracy"),
            "test": head.accuracy(&test_x, &test_y).expect("test accuracy"),
        }),
        None => json!({ "probe": name, "error": "ridge degenerate" }),
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: classifier_context <model.safetensors>");

    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let key = safetensors::read_tensor(&path, KEY).expect("key");
    let value = safetensors::read_tensor(&path, VALUE).expect("value");
    let ln1_w = safetensors::read_vector(&path, LN1_W).expect("ln1 weight");
    let ln1_b = safetensors::read_vector(&path, LN1_B).expect("ln1 bias");
    let ff1_w = safetensors::read_tensor(&path, FF1_W).expect("ff1 weight");
    let ff1_b = safetensors::read_vector(&path, FF1_B).expect("ff1 bias");
    let ff2_w = safetensors::read_tensor(&path, FF2_W).expect("ff2 weight");
    let ff2_b = safetensors::read_vector(&path, FF2_B).expect("ff2 bias");
    let ln2_w = safetensors::read_vector(&path, LN2_W).expect("ln2 weight");
    let ln2_b = safetensors::read_vector(&path, LN2_B).expect("ln2 bias");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = query.cols;
    let block = TransformerBlock::new(
        F32Attention::new(query.values, key.values, value.values, dim, dim).expect("attention"),
        LayerNorm::new(ln1_w, ln1_b).expect("ln1"),
        Dense::new(ff1_w.values, ff1_b, ff1_w.rows, ff1_w.cols).expect("ff1"),
        Dense::new(ff2_w.values, ff2_b, ff2_w.rows, ff2_w.cols).expect("ff2"),
        LayerNorm::new(ln2_w, ln2_b).expect("ln2"),
    )
    .expect("block");

    let samples: Vec<Vec<f32>> = (0..SAMPLES)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();
    let context: Vec<Vec<f32>> = (SAMPLES..SAMPLES + CONTEXT)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();

    let mut rng = Lcg::new(SEED);
    // 标签 = 软加权池化后的固定投影按分位切 4 档（每类样本数相同，且必须读 context）。
    let axis: Vec<f32> = (0..dim).map(|_| rng.next_f32() * 0.1).collect();
    let scores = context_scores(&samples, &context, &axis, TEMPERATURE);
    let labels = rank_labels(&scores, CLASSES);

    let mut label_histogram = vec![0usize; CLASSES];
    for label in &labels {
        label_histogram[*label] += 1;
    }

    let mut order: Vec<usize> = (0..SAMPLES).collect();
    for index in (1..SAMPLES).rev() {
        let draw = rng.next_f32().abs();
        let swap = (draw * (index + 1) as f32) as usize % (index + 1);
        order.swap(index, swap);
    }
    let split = (SAMPLES as f32 * TRAIN_RATIO) as usize;
    let (train, test) = order.split_at(split);

    let outputs = forward_with(&block, &samples, &context).expect("block outputs");
    let concat: Vec<Vec<f32>> = outputs
        .iter()
        .zip(&samples)
        .map(|(out, raw)| {
            let mut joined = out.clone();
            joined.extend_from_slice(raw);
            joined
        })
        .collect();

    let readings = vec![
        probe("raw", &samples, &labels, train, test),
        probe("block", &outputs, &labels, train, test),
        probe("concat", &concat, &labels, train, test),
    ];

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "samples": SAMPLES,
            "context": CONTEXT,
            "train_samples": train.len(),
            "test_samples": test.len(),
            "classes": CLASSES,
            "lambda": LAMBDA,
            "chance": 1.0 / CLASSES as f32,
            "task": "label = quantile of a fixed projection of softmax-pooled context",
            "temperature": TEMPERATURE,
            "label_histogram": label_histogram,
            "probes": readings,
            "finite": concat.iter().flatten().all(|value| value.is_finite()),
        }))
        .expect("json")
    );
}
