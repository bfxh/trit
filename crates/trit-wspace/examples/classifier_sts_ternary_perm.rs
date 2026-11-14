//! 置换能降低三值化的代价吗？——分组共享 scale 的实验。
//!
//! 前置：全部线性权重三值化（per-tensor 单一 scale）在 300 对子集上是 **−24.91 点**。
//!
//! 一个必须先说清的**数学事实**：逐元素三值化 `w → trit · scale` 对**置换完全不变**——
//! 每个元素独立量化，误差只取决于 `(w, scale)`，与它排在哪无关。所以"把重要权重聚到一起
//! 再三值化"在**逐元素**方案下不会有任何效果。
//!
//! 置换真正能起作用的是**分组共享 scale**：每 `G` 个元素共用一个 `E|w|`。若按幅度排序后
//! 分组，组内 magnitude 接近，共享 scale 就更贴合，误差更小——这才是"置换 + 三值"的机理。
//!
//! 四个配置（同一批 300 对子集）：
//!
//! | 配置 | scale 粒度 | 顺序 | 置换是否起作用 |
//! |---|---|---|---|
//! | `per-tensor` | 整张量一个 | — | 否（基线 −24.91） |
//! | `per-row` | 每行一个 | — | 否 |
//! | `group32` | 每 32 个一个 | 原始 | — |
//! | `group32-sorted` | 每 32 个一个 | **按幅度排序** | **是** |
//!
//! 用法：`classifier_sts_ternary_perm <model> <vocab> <test.tsv> [对数]`

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
const GROUP: usize = 32;

/// 一个配置：(名字, 每层做什么)。
type LayerConfig = (&'static str, fn(&mut BertLayer));

fn noop(_: &mut BertLayer) {}

/// 对一组下标按共享 `E|w|` 做三值化。
fn ternary_span(weight: &mut [f32], indices: &[usize]) {
    if indices.is_empty() {
        return;
    }
    let mean_abs = indices
        .iter()
        .map(|index| weight[*index].abs())
        .sum::<f32>()
        / indices.len() as f32;
    if mean_abs == 0.0 {
        return;
    }
    let threshold = 0.7 * mean_abs;
    for index in indices {
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

/// 整张量一个 scale。
fn ternary_per_tensor(weight: &mut [f32]) {
    let indices: Vec<usize> = (0..weight.len()).collect();
    ternary_span(weight, &indices);
}

/// 每行一个 scale（`cols` 是行宽）。
fn ternary_per_row(weight: &mut [f32], cols: usize) {
    for start in (0..weight.len()).step_by(cols.max(1)) {
        let end = (start + cols).min(weight.len());
        let indices: Vec<usize> = (start..end).collect();
        ternary_span(weight, &indices);
    }
}

/// 每 `GROUP` 个共用一个 scale，保持原始顺序。
fn ternary_group(weight: &mut [f32]) {
    for start in (0..weight.len()).step_by(GROUP) {
        let end = (start + GROUP).min(weight.len());
        let indices: Vec<usize> = (start..end).collect();
        ternary_span(weight, &indices);
    }
}

/// 每 `GROUP` 个共用一个 scale，**先按幅度排序再分组**（这就是"置换"的作用点）。
fn ternary_group_sorted(weight: &mut [f32]) {
    let mut order: Vec<usize> = (0..weight.len()).collect();
    order.sort_by(|a, b| {
        weight[*a]
            .abs()
            .partial_cmp(&weight[*b].abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for chunk in order.chunks(GROUP) {
        ternary_span(weight, chunk);
    }
}

/// 一层的 6 个线性权重。
fn each_weight(layer: &mut BertLayer, mut apply: impl FnMut(&mut [f32])) {
    apply(&mut layer.wq.weight);
    apply(&mut layer.wk.weight);
    apply(&mut layer.wv.weight);
    apply(&mut layer.wo.weight);
    apply(&mut layer.ff1.weight);
    apply(&mut layer.ff2.weight);
}

/// 每层按行三值化（行宽从权重本身取，避免闭包捕获）。
fn ternary_row_layer(layer: &mut BertLayer) {
    let cols = layer.wq.cols;
    each_weight(layer, |weight| ternary_per_row(weight, cols));
}

/// 每层整张量三值化。
fn ternary_tensor_layer(layer: &mut BertLayer) {
    each_weight(layer, ternary_per_tensor);
}

/// 每层 group32（原始顺序）。
fn ternary_group_layer(layer: &mut BertLayer) {
    each_weight(layer, ternary_group);
}

/// 每层 group32（按幅度排序后分组 = 置换）。
fn ternary_sorted_layer(layer: &mut BertLayer) {
    each_weight(layer, ternary_group_sorted);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts_ternary_perm <model> <vocab> <test.tsv> [对数]");
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

    // (名字, 每层应用什么)
    let configs: Vec<LayerConfig> = vec![
        ("f32 (baseline)", noop),
        ("ternary per-tensor", ternary_tensor_layer),
        ("ternary per-row", ternary_row_layer),
        ("ternary group32", ternary_group_layer),
        ("ternary group32 sorted", ternary_sorted_layer),
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
            "group": GROUP,
            "note": "逐元素三值化对置换不变；置换只在分组共享 scale 时起作用",
            "readings": readings,
        }))
        .expect("json")
    );
}
