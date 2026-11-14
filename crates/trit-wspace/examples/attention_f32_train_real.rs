//! 真实 f32 位置编码训练：MiniLM 12 头，每头目标 dominant = 头索引。
//!
//! Q/K/V 固定，只对位置偏置做解析梯度下降（交叉熵损失）。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::attention_f32_train::{TrainOptions, train_positions};
use trit_wspace::safetensors::{self, Weights};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const HEADS: usize = 12;
const KEYS: usize = 32;
const ROUNDS: usize = 20;
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
        .expect("用法: attention_f32_train_real <model.safetensors>");
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
    let mut rows = Vec::new();
    for (index, head) in heads.iter().enumerate() {
        let report = train_positions(
            head,
            &input,
            &keys,
            &values,
            &TrainOptions {
                rounds: ROUNDS,
                lr: LR,
                target: index,
            },
        )
        .expect("train");
        let first = report.losses.first().copied().unwrap_or(0.0);
        let last = report.losses.last().copied().unwrap_or(0.0);
        let grad = report.grad_norms.first().copied().unwrap_or(0.0);
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
        rows.push(json!({
            "head": index,
            "target": index,
            "loss_before": first,
            "loss_after": last,
            "grad_norm_first": grad,
            "positions_min": min,
            "positions_max": max,
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "heads": HEADS,
            "head_dim": head_dim,
            "keys": KEYS,
            "rounds": ROUNDS,
            "lr": LR,
            "rows": rows,
        }))
        .expect("json")
    );
}
