//! 头剪枝：按**重要性排序** vs 按**索引截断**，差多少？
//!
//! `classifier_sts_heads` 发现：保留"前 3 个头"（−19.76 点）比"前 1 个头"（−12.83）还差，
//! 说明头的重要性不按索引顺序。本示例把这一点测清楚：
//!
//! - 重要性代理 = `attention.output.dense`（`wo`）中该头对应**列的 L2 范数**
//!   （structured pruning 的常用 filter-norm 做法；纯权重分析，不必改共享 `forward`）；
//! - **每层独立排序**（不同层的头功能不同，不该跨层统一排）；
//! - 对照三种策略：`keep-low`（按索引取前 N）、`keep-ranked`（按范数取前 N）、
//!   以及 `drop-low`（按范数丢掉最弱的 N 个）。
//!
//! 同一批 300 对子集（与 `classifier_sts_heads` 可比）。
//!
//! 用法：`classifier_sts_heads_ranked <model> <vocab> <test.tsv> [对数]`

use serde_json::json;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::LayerNorm;

#[path = "sts/bert.rs"]
#[allow(dead_code)]
mod bert;
#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use bert::{BertLayer, HEADS, bert_sequence};
use common::{Sentence, cosine, encode, load_vocab, mean, read_pairs, spearman};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";

/// 一个配置：(名字, 每层做什么变换)。
type HeadConfig = (&'static str, fn(&mut BertLayer));

/// 每个头在 `wo` 里对应列的 L2 范数（重要性代理）。
fn head_norms(layer: &BertLayer) -> Vec<f32> {
    let head_dim = layer.wo.cols / HEADS;
    (0..HEADS)
        .map(|head| {
            let start = head * head_dim;
            let mut sum = 0.0f32;
            for row in 0..layer.wo.rows {
                for col in start..start + head_dim {
                    let weight = layer.wo.weight[row * layer.wo.cols + col];
                    sum += weight * weight;
                }
            }
            sum.sqrt()
        })
        .collect()
}

/// 把 `drop` 里的头对应的 `wo` 列置零。
fn zero_heads(layer: &mut BertLayer, drop: &[usize]) {
    let head_dim = layer.wo.cols / HEADS;
    for head in drop {
        let start = head * head_dim;
        for row in 0..layer.wo.rows {
            for col in start..start + head_dim {
                layer.wo.weight[row * layer.wo.cols + col] = 0.0;
            }
        }
    }
}

/// 按索引保留前 `keep` 个头。
fn keep_index(layer: &mut BertLayer, keep: usize) {
    let drop: Vec<usize> = (keep..HEADS).collect();
    zero_heads(layer, &drop);
}

/// 按范数保留最强的 `keep` 个头。
fn keep_ranked(layer: &mut BertLayer, keep: usize) {
    let norms = head_norms(layer);
    let mut order: Vec<usize> = (0..HEADS).collect();
    order.sort_by(|a, b| {
        norms[*b]
            .partial_cmp(&norms[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let drop: Vec<usize> = order[keep..].to_vec();
    zero_heads(layer, &drop);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts_heads_ranked <model> <vocab> <test.tsv> [对数]");
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

    // 先打印每层的头范数排序，便于人工核对。
    let probe: Vec<BertLayer> = (0..6).map(|layer| BertLayer::load(&model, layer)).collect();
    let norm_report: Vec<serde_json::Value> = probe
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            let norms = head_norms(layer);
            let mut order: Vec<usize> = (0..HEADS).collect();
            order.sort_by(|a, b| {
                norms[*b]
                    .partial_cmp(&norms[*a])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            json!({
                "layer": index,
                "ranked_head_order": order,
                "norms": norms.iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
            })
        })
        .collect();

    let configs: Vec<HeadConfig> = vec![
        ("12 heads (full)", |_| {}),
        ("index-6 (前 6 个)", |l| keep_index(l, 6)),
        ("ranked-6 (最强 6 个)", |l| keep_ranked(l, 6)),
        ("index-3", |l| keep_index(l, 3)),
        ("ranked-3", |l| keep_ranked(l, 3)),
        ("index-1", |l| keep_index(l, 1)),
        ("ranked-1", |l| keep_ranked(l, 1)),
        ("ranked-2", |l| keep_ranked(l, 2)),
    ];

    let mut readings = Vec::new();
    for (name, transform) in &configs {
        let mut layers: Vec<BertLayer> =
            (0..6).map(|layer| BertLayer::load(&model, layer)).collect();
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
        readings.push(json!({ "config": name, "spearman": spearman(&cosines, &gold) }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "pairs": pairs.len(),
            "dim": dim,
            "importance": "L2 norm of the head's columns in attention.output.dense (per layer)",
            "head_order_by_layer": norm_report,
            "readings": readings,
        }))
        .expect("json")
    );
}
