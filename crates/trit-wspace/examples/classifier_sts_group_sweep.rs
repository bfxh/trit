//! 三值 + 排序分组：扫分组大小，找"精度 vs 压缩率"的 Pareto 前沿。
//!
//! 机理（`classifier_sts_ternary_perm` 已验证）：逐元素三值化对置换不变，真正起作用的是
//! **分组共享 scale**；按幅度排序后每组 magnitude 接近，组 scale 恰好适配，代价从 −24.91
//! 点降到 +0.11 点（group=32）。
//!
//! 本示例扫 `group ∈ {4, 8, 16, 32, 64, 128, 256}`，报 Spearman 与**等效位宽**：
//!
//! ```text
//! bits/weight = 2（trit）+ 32 / group（共享 f32 scale）
//! 压缩率 = 32 / bits
//! ```
//!
//! 用法：`classifier_sts_group_sweep <model> <vocab> <test.tsv> [对数]`

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
const GROUPS: [usize; 7] = [4, 8, 16, 32, 64, 128, 256];

/// 按幅度排序后每 `group` 个共享一个 `E|w|` 做三值化。
fn ternary_sorted(weight: &mut [f32], group: usize) {
    let mut order: Vec<usize> = (0..weight.len()).collect();
    order.sort_by(|a, b| {
        weight[*a]
            .abs()
            .partial_cmp(&weight[*b].abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for chunk in order.chunks(group.max(1)) {
        let mean_abs =
            chunk.iter().map(|index| weight[*index].abs()).sum::<f32>() / chunk.len() as f32;
        if mean_abs == 0.0 {
            continue;
        }
        let threshold = 0.7 * mean_abs;
        for index in chunk {
            let value = weight[*index];
            let trit = if value > threshold {
                1.0
            } else if value < -threshold {
                -1.0
            } else {
                0.0
            };
            weight[*index] = trit * mean_abs;
        }
    }
}

fn each_weight(layer: &mut BertLayer, mut apply: impl FnMut(&mut [f32])) {
    apply(&mut layer.wq.weight);
    apply(&mut layer.wk.weight);
    apply(&mut layer.wv.weight);
    apply(&mut layer.wo.weight);
    apply(&mut layer.ff1.weight);
    apply(&mut layer.ff2.weight);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts_group_sweep <model> <vocab> <test.tsv> [对数]");
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

    let evaluate = |layers: &[BertLayer]| -> f64 {
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
        spearman(&cosines, &gold)
    };

    // 基线（f32）
    let base_layers: Vec<BertLayer> = (0..6).map(|layer| BertLayer::load(&model, layer)).collect();
    let baseline = evaluate(&base_layers);

    let mut readings = vec![json!({
        "config": "f32 (baseline)",
        "bits_per_weight": 32.0,
        "compression": 1.0,
        "spearman": baseline,
        "delta": 0.0,
    })];

    for group in GROUPS {
        let mut layers: Vec<BertLayer> =
            (0..6).map(|layer| BertLayer::load(&model, layer)).collect();
        for layer in layers.iter_mut() {
            each_weight(layer, |weight| ternary_sorted(weight, group));
        }
        let spearman = evaluate(&layers);
        let bits = 2.0 + 32.0 / group as f64;
        readings.push(json!({
            "config": format!("ternary sorted group{group}"),
            "group": group,
            "bits_per_weight": bits,
            "compression": 32.0 / bits,
            "spearman": spearman,
            "delta": spearman - baseline,
        }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "pairs": pairs.len(),
            "dim": dim,
            "note": "bits/weight = 2(trit) + 32/group(shared f32 scale)；压缩率 = 32/bits",
            "readings": readings,
        }))
        .expect("json")
    );
}
