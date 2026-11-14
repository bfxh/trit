//! 真实 MiniLM 的三值位置编码：从 `position_embeddings` 切 12 头 × 32 维。
//!
//! 对比三种情况：无位置信号、线性偏置 `-(i)/4`、三值位置编码。

use std::collections::BTreeMap;

use serde_json::json;
use trit_wspace::attention::AttentionHead;
use trit_wspace::attention_multi::MultiHeadAttention;
use trit_wspace::attention_position::{PositionEncoding, forward_with_encodings};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Trit};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
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

fn position_table(weights: &Weights, start: usize, width: usize) -> Vec<Vec<Trit>> {
    (0..KEYS)
        .map(|position| {
            let offset = position * weights.cols + start;
            trit_row(&weights.values[offset..offset + width])
        })
        .collect()
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
        .expect("用法: attention_position_real <model.safetensors>");
    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let key = safetensors::read_tensor(&path, KEY).expect("key");
    let value = safetensors::read_tensor(&path, VALUE).expect("value");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let positions = safetensors::read_tensor(&path, POSITIONS).expect("positions");
    let dim = query.cols;
    let head_dim = dim / HEADS;
    let query_layer = query.to_layer().expect("q");
    let key_layer = key.to_layer().expect("k");
    let value_layer = value.to_layer().expect("v");
    let mut heads = Vec::with_capacity(HEADS);
    let mut encodings = Vec::with_capacity(HEADS);
    for head in 0..HEADS {
        heads.push(
            AttentionHead::new(
                slice_layer(&query_layer, head * head_dim, head_dim),
                slice_layer(&key_layer, head * head_dim, head_dim),
                slice_layer(&value_layer, head * head_dim, head_dim),
            )
            .expect("head"),
        );
        encodings.push(
            PositionEncoding::new(position_table(&positions, head * head_dim, head_dim))
                .expect("encoding"),
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
        let linear = multi
            .forward(&input, &keys, &values, &position_bias)
            .expect("linear");
        let encoded =
            forward_with_encodings(&multi, &input, &keys, &values, &encodings).expect("encoded");
        let plain_dominant: Vec<usize> = plain.iter().map(|trace| trace.dominant).collect();
        let linear_dominant: Vec<usize> = linear.iter().map(|trace| trace.dominant).collect();
        let encoded_dominant: Vec<usize> = encoded.iter().map(|trace| trace.dominant).collect();
        rows.push(json!({
            "query": index,
            "plain_dominant": plain_dominant,
            "linear_dominant": linear_dominant,
            "encoded_dominant": encoded_dominant,
            "plain_histogram": histogram(&plain_dominant),
            "encoded_histogram": histogram(&encoded_dominant),
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "heads": HEADS,
            "head_dim": head_dim,
            "keys": KEYS,
            "queries": QUERIES,
            "position_rows": positions.rows,
            "rows": rows,
        }))
        .expect("json")
    );
}
