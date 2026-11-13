//! 真实多头联合训练：MiniLM 12 头，Q/K 权重 + 共享位置偏置一起训练。
//!
//! 每头目标 dominant = 头索引；训练后统计匹配数与位置范围。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::attention_f32_joint::{JointOptions, train_joint};
use trit_wspace::safetensors::{self, Weights};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const HEADS: usize = 12;
const KEYS: usize = 32;
const ROUNDS: usize = 100;
const LR: f32 = 0.5;

fn slice_f32(weights: &Weights, start: usize, width: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(weights.rows * width);
    for row in 0..weights.rows {
        let offset = row * weights.cols + start;
        out.extend_from_slice(&weights.values[offset..offset + width]);
    }
    out
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: attention_f32_joint_real <model.safetensors>");
    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let key = safetensors::read_tensor(&path, KEY).expect("key");
    let value = safetensors::read_tensor(&path, VALUE).expect("value");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = query.cols;
    let head_dim = dim / HEADS;
    let mut heads = Vec::with_capacity(HEADS);
    for head in 0..HEADS {
        heads.push(
            F32Attention::new(
                slice_f32(&query, head * head_dim, head_dim),
                slice_f32(&key, head * head_dim, head_dim),
                slice_f32(&value, head * head_dim, head_dim),
                dim,
                head_dim,
            )
            .expect("head"),
        );
    }
    let keys: Vec<Vec<f32>> = (0..KEYS)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();
    let values = keys.clone();
    let input = embeddings.values[..dim].to_vec();
    let targets: Vec<usize> = (0..HEADS).collect();
    let report = train_joint(
        &mut heads,
        &input,
        &keys,
        &JointOptions {
            rounds: ROUNDS,
            lr: LR,
            targets: targets.clone(),
        },
    )
    .expect("train");
    let mut dominant = Vec::with_capacity(HEADS);
    let mut matched = 0usize;
    for (index, head) in heads.iter().enumerate() {
        let trace = head.forward(&input, &keys, &values).expect("forward");
        dominant.push(trace.dominant);
        if trace.dominant == targets[index] {
            matched += 1;
        }
    }
    let min = report
        .positions
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    let max = report
        .positions
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "heads": HEADS,
            "head_dim": head_dim,
            "keys": KEYS,
            "rounds": ROUNDS,
            "lr": LR,
            "loss_before": report.losses.first().copied().unwrap_or(0.0),
            "loss_after": report.losses.last().copied().unwrap_or(0.0),
            "grad_norm_first": report.grad_norms.first().copied().unwrap_or(0.0),
            "matched": matched,
            "dominant": dominant,
            "positions_min": min,
            "positions_max": max,
        }))
        .expect("json")
    );
}
