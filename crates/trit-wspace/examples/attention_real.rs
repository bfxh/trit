//! 真实 MiniLM 的三值注意力：Q/K/V 层三值化，词表行做 keys/values。
//! 输出每个 query 的 dominant key 与 top-5 权重。

use serde_json::json;
use trit_wspace::Trit;
use trit_wspace::attention::AttentionHead;
use trit_wspace::safetensors::{self, Weights};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
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

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: attention_real <model>");
    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let key = safetensors::read_tensor(&path, KEY).expect("key");
    let value = safetensors::read_tensor(&path, VALUE).expect("value");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = query.cols;
    let head = AttentionHead::new(
        query.to_layer().expect("q"),
        key.to_layer().expect("k"),
        value.to_layer().expect("v"),
    )
    .expect("head");
    let keys: Vec<Vec<Trit>> = (0..KEYS)
        .map(|row| trit_row(&embeddings.values[row * dim..(row + 1) * dim]))
        .collect();
    let values = keys.clone();
    let mut rows = Vec::new();
    for index in 0..QUERIES {
        let input = trit_row(&embeddings.values[index * dim..(index + 1) * dim]);
        let trace = head.forward(&input, &keys, &values).expect("forward");
        let mut top: Vec<(usize, f32)> = trace.weights.iter().copied().enumerate().collect();
        top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        rows.push(json!({
            "query": index,
            "dominant": trace.dominant,
            "top": top.iter().take(5).map(|(key, weight)| json!({"key": key, "weight": weight})).collect::<Vec<_>>(),
            "scores": trace.scores.iter().take(8).collect::<Vec<_>>(),
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "keys": KEYS, "queries": QUERIES, "rows": rows,
        }))
        .expect("json")
    );
}
