//! 剪枝 / 量化后 STS 掉多少？——用已验证的流水线量代价。
//!
//! 前置：完整 6 层 + mean pooling 在 STS test 全量上是 **0.8204**（与官方持平）。
//! 这里在同一批 test 子集上（保证可比）对比几种**权重空间**改动：
//!
//! | 配置 | 改了什么 |
//! |---|---|
//! | \`full\` | 不动（基线） |
//! | \`skip-mid\` | 跳过 layer 2/3/4（6 层 → 3 层） |
//! | \`int8\` | 所有权重对称 int8 量化（含反向边界 → 回到 f32） |
//! | \`ternary\` | 所有权重三值化（\`-1/0/+1\` × 每组均值），即本仓的权重空间主题 |
//!
//! 注意：子集上的绝对值与 0.8204 不可直接比，但**相对掉幅**可比。
//!
//! 用法：\`classifier_sts_prune <model> <vocab> <test.tsv> [对数]\`

use serde_json::json;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::LayerNorm;

#[path = "sts/bert.rs"]
#[allow(dead_code)]
mod bert;
#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use bert::{BertLayer, bert_sequence};
use common::{Sentence, cosine, encode, load_vocab, mean, read_pairs, spearman};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";

/// 对称 int8：`w → round(w/scale)·scale`，`scale = max|w| / 127`。
fn quantize_int8(layer: &mut BertLayer) {
    let mut all: Vec<&mut Vec<f32>> = vec![
        &mut layer.wq.weight,
        &mut layer.wk.weight,
        &mut layer.wv.weight,
        &mut layer.wo.weight,
        &mut layer.ff1.weight,
        &mut layer.ff2.weight,
    ];
    for weight in all.iter_mut() {
        let max = weight.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        if max == 0.0 {
            continue;
        }
        let scale = max / 127.0;
        for value in weight.iter_mut() {
            let quantized = (*value / scale).round().clamp(-127.0, 127.0);
            *value = quantized * scale;
        }
    }
}

/// 三值：按每组 `E|w|` 作 scale，阈值 `0.7·E|w|`（TWN 的常见取法）。
fn quantize_ternary(layer: &mut BertLayer) {
    let mut all: Vec<&mut Vec<f32>> = vec![
        &mut layer.wq.weight,
        &mut layer.wk.weight,
        &mut layer.wv.weight,
        &mut layer.wo.weight,
        &mut layer.ff1.weight,
        &mut layer.ff2.weight,
    ];
    for weight in all.iter_mut() {
        let count = weight.len().max(1) as f32;
        let mean_abs = weight.iter().map(|v| v.abs()).sum::<f32>() / count;
        if mean_abs == 0.0 {
            continue;
        }
        let threshold = 0.7 * mean_abs;
        for value in weight.iter_mut() {
            let trit = if *value > threshold {
                1.0
            } else if *value < -threshold {
                -1.0
            } else {
                0.0
            };
            *value = trit * mean_abs;
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts_prune <model> <vocab> <test.tsv> [对数]");
    let vocab_path = args.next().expect("vocab.txt");
    let tsv_path = args.next().expect("test.tsv");
    let pairs_count: usize = args
        .next()
        .map(|value| value.parse().expect("对数"))
        .unwrap_or(300);

    let vocab = load_vocab(&vocab_path);
    let embeddings = safetensors::read_tensor(&model, EMBEDDINGS).expect("embeddings");
    let position = safetensors::read_tensor(&model, POSITIONS).expect("positions");
    let token_type = safetensors::read_tensor(&model, TOKEN_TYPE).expect("token type");
    let emb_ln = LayerNorm::new(
        safetensors::read_vector(&model, EMB_LN_W).expect("emb ln w"),
        safetensors::read_vector(&model, EMB_LN_B).expect("emb ln b"),
    )
    .expect("emb ln");
    let dim = embeddings.cols;

    let pairs = read_pairs(&tsv_path, pairs_count);
    let gold: Vec<f64> = pairs.iter().map(|(_, _, score)| *score).collect();
    // 预先编码成 embedding 序列，各配置共用（避免重复分词）。
    let encoded: Vec<(Sentence, Sentence)> = pairs
        .iter()
        .map(|(left, right, _)| {
            let build = |text: &str| -> Sentence {
                let ids = encode(text, &vocab);
                bert_sequence(&ids, &embeddings, &position, &token_type, &emb_ln, dim)
            };
            (build(left), build(right))
        })
        .collect();

    let evaluate = |layers: &[BertLayer], label: &str| -> serde_json::Value {
        let cosines: Vec<f64> = encoded
            .iter()
            .map(|(left, right)| {
                let run = |sequence: &Sentence| -> Vec<f32> {
                    let mut current = sequence.clone();
                    for layer in layers {
                        current = layer.forward_cached(&current).0;
                    }
                    mean(&current, dim)
                };
                cosine(&run(left), &run(right))
            })
            .collect();
        json!({ "config": label, "layers": layers.len(), "spearman": spearman(&cosines, &gold) })
    };

    let mut readings = Vec::new();

    // 1) 基线：6 层全用
    let base: Vec<BertLayer> = (0..6).map(|layer| BertLayer::load(&model, layer)).collect();
    readings.push(evaluate(&base, "full (6 layers)"));

    // 2) 剪枝：跳过 layer 2/3/4
    let skip: Vec<BertLayer> = base
        .iter()
        .enumerate()
        .filter(|(index, _)| !matches!(index, 2..=4))
        .map(|(index, _)| BertLayer::load(&model, index))
        .collect();
    readings.push(evaluate(&skip, "skip layer 2/3/4"));

    // 3) int8 量化
    let mut int8: Vec<BertLayer> = (0..6).map(|layer| BertLayer::load(&model, layer)).collect();
    for layer in int8.iter_mut() {
        quantize_int8(layer);
    }
    readings.push(evaluate(&int8, "int8 (all linear weights)"));

    // 4) 三值量化
    let mut ternary: Vec<BertLayer> = (0..6).map(|layer| BertLayer::load(&model, layer)).collect();
    for layer in ternary.iter_mut() {
        quantize_ternary(layer);
    }
    readings.push(evaluate(&ternary, "ternary (all linear weights)"));

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "pairs": pairs.len(),
            "dim": dim,
            "note": "子集上的绝对值与全量 0.8204 不可直接比，但相对掉幅可比",
            "readings": readings,
        }))
        .expect("json")
    );
}
