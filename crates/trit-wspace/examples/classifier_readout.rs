//! 块输出到底有没有**增量**信息？——三个探针的闭式对比。
//!
//! 已知（`classifier_probe`）：留出集上原始词向量 0.3945、冻结块输出 0.3672，
//! 块还不如原始向量。但那只能说明"块单独读出更差"，不能说明"块没有新信息"。
//!
//! 本示例把三个读出放在同一套切分上比较：
//! 1. `raw`     ：原始词向量（384 维）
//! 2. `block`   ：冻结块输出（384 维）
//! 3. `concat`  ：两者拼接（768 维）——**若明显高于 max(1,2)，说明块有增量信息；
//!    若与 `raw` 持平或更低，说明块只在稀释原始信号。**
//!
//! 全部用闭式岭回归（`ClassifierHead::fit_ridge`），不涉及训练循环。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::classifier_head::ClassifierHead;
use trit_wspace::safetensors;
use trit_wspace::teacher_f32::{Lcg, quad_labels};
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

fn pick(features: &[Vec<f32>], indices: &[usize]) -> Vec<Vec<f32>> {
    indices
        .iter()
        .map(|index| features[*index].clone())
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
        .expect("用法: classifier_readout <model.safetensors>");

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
    let labels = quad_labels(&mut rng, &samples, CLASSES);
    let mut histogram = vec![0usize; CLASSES];
    for label in &labels {
        histogram[*label] += 1;
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
            "label_histogram": histogram,
            "probes": readings,
            "finite": concat.iter().flatten().all(|value| value.is_finite()),
        }))
        .expect("json")
    );
}
