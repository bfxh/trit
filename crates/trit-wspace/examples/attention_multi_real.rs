//! 真实 MiniLM 的多头三值注意力：12 头 × 32 维，128 个 key，位置偏置对照。
//!
//! 输出每个 query 在「无偏置 / 有位置偏置」下每头的 dominant key，
//! 以及 dominant 的直方图——看注意力是否稳定地集中在少数 key。

use std::collections::BTreeMap;

use serde_json::json;
use trit_wspace::attention::AttentionHead;
use trit_wspace::attention_multi::MultiHeadAttention;
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Trit};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const HEADS: usize = 12;
const KEYS: usize = 128;
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

fn histogram(dominants: &[usize]) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for dominant in dominants {
        *map.entry(dominant.to_string()).or_insert(0) += 1;
    }
    map
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: attention_multi_real <model.safetensors>");
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
    let position_bias: Vec<i32> = (0..KEYS).map(|index| -(index as i32) / 4).collect();
    let mut rows = Vec::new();
    for index in 0..QUERIES {
        let input = trit_row(&embeddings.values[index * dim..(index + 1) * dim]);
        let plain = multi.forward(&input, &keys, &values, &[]).expect("plain");
        let biased = multi
            .forward(&input, &keys, &values, &position_bias)
            .expect("biased");
        let plain_dominant: Vec<usize> = plain.iter().map(|trace| trace.dominant).collect();
        let biased_dominant: Vec<usize> = biased.iter().map(|trace| trace.dominant).collect();
        rows.push(json!({
            "query": index,
            "plain_dominant": plain_dominant,
            "biased_dominant": biased_dominant,
            "plain_histogram": histogram(&plain_dominant),
            "biased_histogram": histogram(&biased_dominant),
            "plain_top_weights": plain.iter().take(3).map(|trace| {
                trace.weights[trace.dominant]
            }).collect::<Vec<_>>(),
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "heads": HEADS,
            "head_dim": head_dim,
            "keys": KEYS,
            "queries": QUERIES,
            "position_bias": "-(index)/4",
            "rows": rows,
        }))
        .expect("json")
    );
}
