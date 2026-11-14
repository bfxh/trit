//! 真实 MiniLM 一层的 transformer 反向传播训练：4 个 token 自编码。
//!
//! 固定注意力/LayerNorm/FFN1，只训练 FFN 第二层；损失是输出与输入的 MSE。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_train::{TrainOptions, train_ffn2};

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
const ROUNDS: usize = 50;
const LR: f32 = 0.01;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: transformer_f32_train_real <model.safetensors>");
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
    let mut block = TransformerBlock::new(
        F32Attention::new(query.values, key.values, value.values, dim, dim).expect("attention"),
        LayerNorm::new(ln1_w, ln1_b).expect("ln1"),
        Dense::new(ff1_w.values, ff1_b, ff1_w.rows, ff1_w.cols).expect("ff1"),
        Dense::new(ff2_w.values, ff2_b, ff2_w.rows, ff2_w.cols).expect("ff2"),
        LayerNorm::new(ln2_w, ln2_b).expect("ln2"),
    )
    .expect("block");
    let tokens: Vec<Vec<f32>> = (0..TOKENS)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();
    let targets = tokens.clone();
    let report = train_ffn2(
        &mut block,
        &tokens,
        &targets,
        &TrainOptions {
            rounds: ROUNDS,
            lr: LR,
        },
    )
    .expect("train");
    let out = block.forward(&tokens).expect("forward");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "tokens": TOKENS,
            "rounds": ROUNDS,
            "lr": LR,
            "loss_before": report.losses.first().copied().unwrap_or(0.0),
            "loss_after": report.losses.last().copied().unwrap_or(0.0),
            "finite": out.iter().flatten().all(|value| value.is_finite()),
            "output_norms": out.iter().map(|token| {
                token.iter().map(|value| value * value).sum::<f32>().sqrt()
            }).collect::<Vec<_>>(),
        }))
        .expect("json")
    );
}
