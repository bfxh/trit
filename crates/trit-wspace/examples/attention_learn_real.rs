//! 从头学习三值位置编码：MiniLM 12 头，目标 dominant [0..11]。
//!
//! 每头独立位置表、初始全零；贪心翻转 trit 直到匹配目标或没有提升。

use serde_json::json;
use trit_wspace::attention::AttentionHead;
use trit_wspace::attention_learn::{LearnOptions, learn_positions};
use trit_wspace::attention_multi::MultiHeadAttention;
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Trit};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const HEADS: usize = 12;
const KEYS: usize = 32;
const QUERIES: usize = 4;

fn trit_row(values: &[f32]) -> Vec<Trit> {
    let weights = Weights {
        rows: 1,
        cols: values.len(),
        values: values.to_vec(),
    };
    let layer = weights.to_layer().expect("ternarize");
    (0..values.len())
        .map(|col| layer.trit(0, col).expect("trit"))
        .collect()
}

fn slice_layer(layer: &Layer, start: usize, width: usize) -> Layer {
    let mut trits = Vec::with_capacity(layer.rows() * width);
    for row in 0..layer.rows() {
        for col in start..start + width {
            trits.push(layer.trit(row, col).unwrap_or(Trit::Zero).to_i8());
        }
    }
    let bias: Vec<i8> = layer.bias().iter().map(|trit| trit.to_i8()).collect();
    Layer::new(layer.rows(), width, &trits, &bias).expect("slice layer")
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: attention_learn_real <model.safetensors>");
    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let key = safetensors::read_tensor(&path, KEY).expect("key");
    let value = safetensors::read_tensor(&path, VALUE).expect("value");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = query.cols;
    let head_dim = dim / HEADS;
    let query_layer = query.to_layer().expect("q");
    let key_layer = key.to_layer().expect("k");
    let value_layer = value.to_layer().expect("v");
    let mut heads = Vec::with_capacity(HEADS);
    for head in 0..HEADS {
        heads.push(
            AttentionHead::new(
                slice_layer(&query_layer, head * head_dim, head_dim),
                slice_layer(&key_layer, head * head_dim, head_dim),
                slice_layer(&value_layer, head * head_dim, head_dim),
            )
            .expect("head"),
        );
    }
    let multi = MultiHeadAttention::new(heads).expect("multi");
    let keys: Vec<Vec<Trit>> = (0..KEYS)
        .map(|row| trit_row(&embeddings.values[row * dim..(row + 1) * dim]))
        .collect();
    let values = keys.clone();
    let targets: Vec<usize> = (0..HEADS).collect();
    let options = LearnOptions {
        max_rounds: 4,
        max_positions: 8,
        max_dims: 8,
    };
    let mut rows = Vec::new();
    for index in 0..QUERIES {
        let input = trit_row(&embeddings.values[index * dim..(index + 1) * dim]);
        let report =
            learn_positions(&multi, &input, &keys, &values, &targets, &options).expect("learn");
        let positions: Vec<usize> = report
            .tables
            .iter()
            .map(|table| {
                table
                    .table
                    .iter()
                    .filter(|row| row.iter().any(|trit| *trit != Trit::Zero))
                    .count()
            })
            .collect();
        rows.push(json!({
            "query": index,
            "matched": report.matched,
            "total": report.total,
            "rounds": report.rounds,
            "converged": report.converged,
            "positions_per_head": positions,
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "heads": HEADS,
            "head_dim": head_dim,
            "keys": KEYS,
            "targets": targets,
            "max_rounds": options.max_rounds,
            "max_positions": options.max_positions,
            "max_dims": options.max_dims,
            "rows": rows,
        }))
        .expect("json")
    );
}
