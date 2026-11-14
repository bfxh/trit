//! 注意力头剪枝：12 → 9 → 6 → 3 → 1 个头，STS 掉多少？
//!
//! 实现技巧：`attention.output.dense`（`wo`）的**列**直接对应 12 个头拼接后的输出，
//! 所以"不用某些头"在数值上等价于把 `wo` 对应列**置零**——不需要改共享的 `forward`。
//!
//! 同一批 300 对子集（可与 `classifier_sts_quant_scope` 的读数横向比；绝对值不与全量
//! 0.8204 直接比）。
//!
//! 用法：`classifier_sts_heads <model> <vocab> <test.tsv> [对数]`

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

/// 只保留前 `keep` 个头：把 `wo` 里其余头对应的列置零。
fn keep_heads(layer: &mut BertLayer, keep: usize) {
    let head_dim = layer.wo.cols / HEADS;
    for head in keep..HEADS {
        let start = head * head_dim;
        for row in 0..layer.wo.rows {
            for col in start..start + head_dim {
                layer.wo.weight[row * layer.wo.cols + col] = 0.0;
            }
        }
    }
}

fn noop(_: &mut BertLayer) {}
fn heads9(l: &mut BertLayer) {
    keep_heads(l, 9);
}
fn heads6(l: &mut BertLayer) {
    keep_heads(l, 6);
}
fn heads3(l: &mut BertLayer) {
    keep_heads(l, 3);
}
fn heads1(l: &mut BertLayer) {
    keep_heads(l, 1);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts_heads <model> <vocab> <test.tsv> [对数]");
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

    let configs: Vec<HeadConfig> = vec![
        ("12 heads (full)", noop),
        ("9 heads", heads9),
        ("6 heads", heads6),
        ("3 heads", heads3),
        ("1 head", heads1),
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
            "note": "子集上的绝对值与全量 0.8204 不可直接比，但相对掉幅可比",
            "readings": readings,
        }))
        .expect("json")
    );
}
