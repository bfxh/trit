//! 真实预测任务：4 个 token，全参数训练预测第 0 个 token 的向量。
//!
//! 与自编码不同，目标不是输入本身；输出 loss 与 384 维符号准确率。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_full::{TrainOptions, train_full};

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
const DEFAULT_ROUNDS: usize = 50;
const DEFAULT_LR: f32 = 1e-3;

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let ln = left.iter().map(|v| v * v).sum::<f32>().sqrt();
    let rn = right.iter().map(|v| v * v).sum::<f32>().sqrt();
    if ln == 0.0 || rn == 0.0 {
        0.0
    } else {
        dot / (ln * rn)
    }
}

fn sign_match(output: &[f32], target: &[f32]) -> f32 {
    let hits = output
        .iter()
        .zip(target)
        .filter(|(a, b)| a.signum() == b.signum())
        .count();
    hits as f32 / output.len() as f32
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("用法: transformer_f32_task_real <model.safetensors> [rounds] [lr]");
    let rounds: usize = args
        .next()
        .map(|value| value.parse().expect("rounds"))
        .unwrap_or(DEFAULT_ROUNDS);
    let lr: f32 = args
        .next()
        .map(|value| value.parse().expect("lr"))
        .unwrap_or(DEFAULT_LR);
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
    let targets = vec![tokens[0].clone(); TOKENS];
    let report =
        train_full(&mut block, &tokens, &targets, &TrainOptions { rounds, lr }).expect("train");
    let out = block.forward(&tokens).expect("forward");
    let cosines: Vec<f32> = out
        .iter()
        .zip(&targets)
        .map(|(o, t)| cosine(o, t))
        .collect();
    let matches: Vec<f32> = out
        .iter()
        .zip(&targets)
        .map(|(o, t)| sign_match(o, t))
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "tokens": TOKENS,
            "rounds": rounds,
            "lr": lr,
            "task": "predict token 0 vector",
            "loss_before": report.losses.first().copied().unwrap_or(0.0),
            "loss_after": report.losses.last().copied().unwrap_or(0.0),
            "cosines": cosines,
            "sign_match": matches,
            "output_norms": out
                .iter()
                .map(|token| token.iter().map(|value| value * value).sum::<f32>().sqrt())
                .collect::<Vec<_>>(),
            "target_norm": targets[0].iter().map(|value| value * value).sum::<f32>().sqrt(),
            "finite": out.iter().flatten().all(|value| value.is_finite()),
        }))
        .expect("json")
    );
}
