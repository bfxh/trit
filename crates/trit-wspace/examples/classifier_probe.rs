//! 冻结 block + 线性头能解码多少？——闭式岭回归 + 留出集。
//!
//! 动机：早期 32 样本 / 1536 头参数下"准确率 1.0"其实是**记忆**（d ≫ N 时线性头能
//! 打散任意标注）。这里把样本数提到 1024（768 训练 / 256 留出，d=384），用闭式解拟合，
//! 同时报**训练集**与**留出集**准确率——两者差距就是记忆的量。
//!
//! 两个探针：
//! 1. `raw_embedding`：原始词向量上的线性探针（二次型教师在原始空间不可线性解码）；
//! 2. `frozen_block`：**冻结 block 输出**上的线性探针（本示例要量的问题）。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::classifier_head::ClassifierHead;
use trit_wspace::safetensors;
use trit_wspace::teacher_f32::{Lcg, quad_labels};
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};

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
const CLASSES: usize = 4;
const TRAIN_RATIO: f32 = 0.75;
const LAMBDA: f32 = 1e-2;
const SEED: u64 = 20261112;

fn probe(
    name: &str,
    features: &[Vec<f32>],
    labels: &[usize],
    train: &[usize],
    test: &[usize],
) -> serde_json::Value {
    let train_x: Vec<Vec<f32>> = train.iter().map(|index| features[*index].clone()).collect();
    let train_y: Vec<usize> = train.iter().map(|index| labels[*index]).collect();
    let test_x: Vec<Vec<f32>> = test.iter().map(|index| features[*index].clone()).collect();
    let test_y: Vec<usize> = test.iter().map(|index| labels[*index]).collect();
    match ClassifierHead::fit_ridge(&train_x, &train_y, LAMBDA) {
        Some(head) => {
            let train_acc = head.accuracy(&train_x, &train_y).expect("train accuracy");
            let test_acc = head.accuracy(&test_x, &test_y).expect("test accuracy");
            json!({
                "probe": name,
                "train_accuracy": train_acc,
                "test_accuracy": test_acc,
            })
        }
        None => json!({ "probe": name, "error": "ridge solve degenerate" }),
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: classifier_probe <model.safetensors>");

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

    let tokens: Vec<Vec<f32>> = (0..SAMPLES)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();

    let mut rng = Lcg::new(SEED);
    let labels = quad_labels(&mut rng, &tokens, CLASSES);
    let mut histogram = vec![0usize; CLASSES];
    for label in &labels {
        histogram[*label] += 1;
    }

    // 确定性切分：洗牌后前 75% 训练、后 25% 留出。
    let mut order: Vec<usize> = (0..SAMPLES).collect();
    for index in (1..SAMPLES).rev() {
        let draw = rng.next_f32().abs();
        let swap = (draw * (index + 1) as f32) as usize % (index + 1);
        order.swap(index, swap);
    }
    let split = (SAMPLES as f32 * TRAIN_RATIO) as usize;
    let (train, test) = order.split_at(split);

    let outputs = block.forward(&tokens).expect("forward");
    let readings = vec![
        probe("raw_embedding", &tokens, &labels, train, test),
        probe("frozen_block", &outputs, &labels, train, test),
    ];

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "samples": SAMPLES,
            "train_samples": train.len(),
            "test_samples": test.len(),
            "classes": CLASSES,
            "lambda": LAMBDA,
            "label_histogram": histogram,
            "chance": 1.0 / CLASSES as f32,
            "teacher": "quad (rank labels)",
            "probes": readings,
            "finite": outputs.iter().flatten().all(|value| value.is_finite()),
        }))
        .expect("json")
    );
}
