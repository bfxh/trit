//! 真实 MiniLM 的 f32 注意力：12 头 × 32 维、128 个 key，与三值版对照。
//!
//! 输出每个 query 每头的 dominant key；三值版的读数在 `attention_multi_real`。

use std::collections::BTreeMap;

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::safetensors::{self, Weights};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const HEADS: usize = 12;
const KEYS: usize = 128;
const QUERIES: usize = 4;

fn slice_f32(weights: &Weights, start: usize, width: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(weights.rows * width);
    for row in 0..weights.rows {
        let offset = row * weights.cols + start;
        out.extend_from_slice(&weights.values[offset..offset + width]);
    }
    out
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
        .expect("用法: attention_f32_real <model.safetensors>");
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
    let mut rows = Vec::new();
    for index in 0..QUERIES {
        let input = embeddings.values[index * dim..(index + 1) * dim].to_vec();
        let mut dominant = Vec::with_capacity(HEADS);
        let mut top_weights = Vec::with_capacity(HEADS);
        for head in &heads {
            let trace = head.forward(&input, &keys, &values).expect("forward");
            dominant.push(trace.dominant);
            top_weights.push(trace.weights[trace.dominant]);
        }
        rows.push(json!({
            "query": index,
            "dominant": dominant,
            "histogram": histogram(&dominant),
            "top_weights": top_weights,
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "heads": HEADS,
            "head_dim": head_dim,
            "keys": KEYS,
            "queries": QUERIES,
            "variant": "f32",
            "rows": rows,
        }))
        .expect("json")
    );
}
