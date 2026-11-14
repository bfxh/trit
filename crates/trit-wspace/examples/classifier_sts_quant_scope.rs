//! 量化拆到子模块：到底是 attention 还是 FFN 对精度更敏感？
//!
//! 前置（`classifier_sts_prune`）：同一批 300 对子集上，全部线性权重 int8 只掉
//! 0.35 点，三值掉 24.9 点。这里把量化**拆到子模块**，定位敏感源：
//!
//! | 配置 | 量化范围 |
//! |---|---|
//! | \`full\` | 不动（基线） |
//! | \`int8-all\` / \`ternary-all\` | 全部 6 个线性权重 |
//! | \`int8-attn\` / \`ternary-attn\` | 只 Q/K/V/out（FFN 保持 f32） |
//! | \`int8-ffn\` / \`ternary-ffn\` | 只 FF1/FF2（attention 保持 f32） |
//! | \`skip-mid\` | 跳过 layer 2/3/4（层数对照） |
//!
//! 用法：\`classifier_sts_quant_scope <model> <vocab> <test.tsv> [对数]\`

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
const ALL: [usize; 6] = [0, 1, 2, 3, 4, 5];

/// 一个配置：(名字, 用哪些层, 每层做什么变换)。
type Config = (&'static str, Vec<usize>, fn(&mut BertLayer));

/// 对称 int8：`w → round(w/max)·max`，`max = max|w|/127`（单张量）。
fn int8(weight: &mut [f32]) {
    let max = weight.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
    if max == 0.0 {
        return;
    }
    let scale = max / 127.0;
    for value in weight.iter_mut() {
        *value = (*value / scale).round().clamp(-127.0, 127.0) * scale;
    }
}

/// 三值：`w → trit · E|w|`，阈值 `0.7·E|w|`。
fn ternary(weight: &mut [f32]) {
    let count = weight.len().max(1) as f32;
    let mean_abs = weight.iter().map(|v| v.abs()).sum::<f32>() / count;
    if mean_abs == 0.0 {
        return;
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

fn noop(_: &mut BertLayer) {}
fn int8_all(l: &mut BertLayer) {
    for w in [&mut l.wq.weight, &mut l.wk.weight, &mut l.wv.weight] {
        int8(w);
    }
    int8(&mut l.wo.weight);
    int8(&mut l.ff1.weight);
    int8(&mut l.ff2.weight);
}
fn int8_attn(l: &mut BertLayer) {
    for w in [&mut l.wq.weight, &mut l.wk.weight, &mut l.wv.weight] {
        int8(w);
    }
    int8(&mut l.wo.weight);
}
fn int8_ffn(l: &mut BertLayer) {
    int8(&mut l.ff1.weight);
    int8(&mut l.ff2.weight);
}
fn ternary_all(l: &mut BertLayer) {
    for w in [&mut l.wq.weight, &mut l.wk.weight, &mut l.wv.weight] {
        ternary(w);
    }
    ternary(&mut l.wo.weight);
    ternary(&mut l.ff1.weight);
    ternary(&mut l.ff2.weight);
}
fn ternary_attn(l: &mut BertLayer) {
    for w in [&mut l.wq.weight, &mut l.wk.weight, &mut l.wv.weight] {
        ternary(w);
    }
    ternary(&mut l.wo.weight);
}
fn ternary_ffn(l: &mut BertLayer) {
    ternary(&mut l.ff1.weight);
    ternary(&mut l.ff2.weight);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts_quant_scope <model> <vocab> <test.tsv> [对数]");
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

    // (名字, 用哪些层, 每层做什么变换)
    let configs: Vec<Config> = vec![
        ("full (f32)", ALL.to_vec(), noop),
        ("int8-all", ALL.to_vec(), int8_all),
        ("int8-attn only", ALL.to_vec(), int8_attn),
        ("int8-ffn only", ALL.to_vec(), int8_ffn),
        ("ternary-all", ALL.to_vec(), ternary_all),
        ("ternary-attn only", ALL.to_vec(), ternary_attn),
        ("ternary-ffn only", ALL.to_vec(), ternary_ffn),
        ("skip layer 2/3/4", vec![0, 1, 5], noop),
    ];

    let mut readings = Vec::new();
    for (name, layer_indices, transform) in &configs {
        let mut layers: Vec<BertLayer> = layer_indices
            .iter()
            .map(|index| BertLayer::load(&model, *index))
            .collect();
        for layer in layers.iter_mut() {
            transform(layer);
        }
        let cosines: Vec<f64> = encoded
            .iter()
            .map(|(left, right)| {
                let run = |sequence: &Sentence| -> Vec<f32> {
                    let mut current = sequence.clone();
                    for layer in &layers {
                        current = layer.forward_cached(&current).0;
                    }
                    mean(&current, dim)
                };
                cosine(&run(left), &run(right))
            })
            .collect();
        readings.push(json!({
            "config": name,
            "layers": layers.len(),
            "spearman": spearman(&cosines, &gold),
        }));
    }

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
