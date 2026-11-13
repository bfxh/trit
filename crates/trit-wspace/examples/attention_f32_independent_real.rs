//! 真实 MiniLM 的每头独立位置训练：12 头、32 key、目标 [0..11]。
//!
//! 与共享位置的联合训练（最多 11/12）对照，看独立位置能否到 12/12。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::attention_f32_independent::{IndependentOptions, train_independent};
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
        .expect("用法: attention_f32_independent_real <model.safetensors>");
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
    let report = train_independent(
        &heads,
        &input,
        &keys,
        &values,
        &IndependentOptions {
            rounds: ROUNDS,
            lr: LR,
            targets: targets.clone(),
        },
    )
    .expect("train");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "heads": HEADS,
            "head_dim": head_dim,
            "keys": KEYS,
            "rounds": ROUNDS,
            "lr": LR,
            "matched": report.matched,
            "total": report.total,
            "detail": report.heads.iter().map(|head| json!({
                "head": head.head,
                "loss_before": head.loss_before,
                "loss_after": head.loss_after,
                "dominant": head.dominant,
                "matched": head.matched,
            })).collect::<Vec<_>>(),
        }))
        .expect("json")
    );
}
