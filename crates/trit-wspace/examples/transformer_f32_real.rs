//! 真实 MiniLM 一层的 f32 transformer 前向：注意力 + 残差 + LN + FFN。
//!
//! 权重全部来自模型：Q/K/V、两个 LayerNorm、intermediate/output dense。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::safetensors;
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
const TOKENS: usize = 4;

fn norm(values: &[f32]) -> f32 {
    values.iter().map(|value| value * value).sum::<f32>().sqrt()
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: transformer_f32_real <model.safetensors>");
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
        F32Attention::new(
            query.values.clone(),
            key.values.clone(),
            value.values.clone(),
            dim,
            dim,
        )
        .expect("attention"),
        LayerNorm::new(ln1_w, ln1_b).expect("ln1"),
        Dense::new(ff1_w.values, ff1_b, ff1_w.rows, ff1_w.cols).expect("ff1"),
        Dense::new(ff2_w.values, ff2_b, ff2_w.rows, ff2_w.cols).expect("ff2"),
        LayerNorm::new(ln2_w, ln2_b).expect("ln2"),
    )
    .expect("block");
    let tokens: Vec<Vec<f32>> = (0..TOKENS)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();
    let out = block.forward(&tokens).expect("forward");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "tokens": TOKENS,
            "input_norms": tokens.iter().map(|token| norm(token)).collect::<Vec<_>>(),
            "output_norms": out.iter().map(|token| norm(token)).collect::<Vec<_>>(),
            "finite": out.iter().flatten().all(|value| value.is_finite()),
            "output_shape": [out.len(), out.first().map_or(0, Vec::len)],
        }))
        .expect("json")
    );
}
