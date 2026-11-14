//! 给关键对比配上**置信区间与成对显著性检验**。
//!
//! 这一路所有结论都以单点 Spearman 表述，比如"三值分组只掉 0.0004 点"。问题是：
//! **这个数在 300/1379 对样本上的抽样误差有多大？**没有区间就没法判断它是真实差异还是噪声。
//!
//! 本示例用 [`trit_wspace::stats`] 给出三样东西：
//! 1. 每个配置的 **95% percentile bootstrap 置信区间**；
//! 2. 两配置差异的**成对 bootstrap p 值**（同一批样本上重采样，消掉样本难度）；
//! 3. 一个**分层采样**的子集，验证子集读数与全量是否一致。
//!
//! 用法：`classifier_sts_significance <model> <vocab> <test.tsv> [对数] [group] [resamples]`

use std::collections::HashMap;

use serde_json::json;
use trit_wspace::grouped::GroupedLayer;
use trit_wspace::safetensors;
use trit_wspace::stats::{bootstrap_spearman_ci, paired_bootstrap_p, spearman, stratified_indices};
use trit_wspace::transformer_f32::{Dense, LayerNorm};

#[path = "sts/bert.rs"]
#[allow(dead_code)]
mod bert;
#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use bert::BertLayer;
use common::{Sentence, cosine, encode, load_vocab, mean, read_pairs};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";

const LINEAR: [&str; 6] = [
    "attention.self.query.weight",
    "attention.self.key.weight",
    "attention.self.value.weight",
    "attention.output.dense.weight",
    "intermediate.dense.weight",
    "output.dense.weight",
];

/// 把整层 6 个线性权重量化成分组三值（替换成等效 f32）。
fn quantize_layer(layer: &mut BertLayer, model: &str, index: usize, group: usize) {
    for suffix in LINEAR {
        let name = format!("encoder.layer.{index}.{suffix}");
        let weight = safetensors::read_tensor(model, &name).expect("weight");
        let bias =
            safetensors::read_vector(model, &name.replace(".weight", ".bias")).expect("bias");
        let quantized =
            GroupedLayer::quantize(weight.rows, weight.cols, &weight.values, &bias, group)
                .expect("quantize");
        let replacement =
            Dense::new(quantized.to_f32(), bias, weight.rows, weight.cols).expect("dense");
        match suffix {
            "attention.self.query.weight" => layer.wq = replacement,
            "attention.self.key.weight" => layer.wk = replacement,
            "attention.self.value.weight" => layer.wv = replacement,
            "attention.output.dense.weight" => layer.wo = replacement,
            "intermediate.dense.weight" => layer.ff1 = replacement,
            _ => layer.ff2 = replacement,
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args.next().expect(
        "用法: classifier_sts_significance <model> <vocab> <tsv> [对数] [group] [resamples]",
    );
    let vocab_path = args.next().expect("vocab.txt");
    let tsv_path = args.next().expect("test.tsv");
    let pairs_count: usize = args
        .next()
        .map(|v| v.parse().expect("pairs"))
        .unwrap_or(300);
    let group: usize = args
        .next()
        .map(|v| v.parse().expect("group"))
        .unwrap_or(256);
    let resamples: usize = args
        .next()
        .map(|v| v.parse().expect("resamples"))
        .unwrap_or(2000);

    let vocab: HashMap<String, usize> = load_vocab(&vocab_path);
    let embeddings = safetensors::read_tensor(&model, EMBEDDINGS).expect("embeddings");
    let position = safetensors::read_tensor(&model, POSITIONS).expect("positions");
    let token_type = safetensors::read_tensor(&model, TOKEN_TYPE).expect("token type");
    let emb_ln = LayerNorm::new(
        safetensors::read_vector(&model, EMB_LN_W).expect("w"),
        safetensors::read_vector(&model, EMB_LN_B).expect("b"),
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
                bert::bert_sequence(&ids, &embeddings, &position, &token_type, &emb_ln, dim)
            };
            (build(left), build(right))
        })
        .collect();

    let cosines_of = |layers: &[BertLayer]| -> Vec<f64> {
        encoded
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
            .collect()
    };

    let baseline_layers: Vec<BertLayer> = (0..6).map(|i| BertLayer::load(&model, i)).collect();
    let baseline = cosines_of(&baseline_layers);
    let mut quantized_layers: Vec<BertLayer> = (0..6).map(|i| BertLayer::load(&model, i)).collect();
    for (index, layer) in quantized_layers.iter_mut().enumerate() {
        quantize_layer(layer, &model, index, group);
    }
    let quantized = cosines_of(&quantized_layers);

    let point_base = spearman(&baseline, &gold);
    let point_quant = spearman(&quantized, &gold);
    let ci_base = bootstrap_spearman_ci(&baseline, &gold, resamples, 0.95, 20261114);
    let ci_quant = bootstrap_spearman_ci(&quantized, &gold, resamples, 0.95, 20261114);
    // 成对检验：P(quantized − baseline <= 0)
    let p_value = paired_bootstrap_p(&baseline, &quantized, &gold, resamples, 20261114);

    // 分层采样：按分数档分成 5 层，抽 40%，看子集读数是否仍接近全量
    let strata: Vec<usize> = gold
        .iter()
        .map(|value| ((value / 5.0) * 5.0).clamp(0.0, 4.0) as usize)
        .collect();
    let subset = stratified_indices(&strata, 0.4, 7);
    let subset_base = {
        let xs: Vec<f64> = subset.iter().map(|i| baseline[*i]).collect();
        let ys: Vec<f64> = subset.iter().map(|i| gold[*i]).collect();
        spearman(&xs, &ys)
    };
    let subset_quant = {
        let xs: Vec<f64> = subset.iter().map(|i| quantized[*i]).collect();
        let ys: Vec<f64> = subset.iter().map(|i| gold[*i]).collect();
        spearman(&xs, &ys)
    };
    let mut layer_counts = std::collections::BTreeMap::new();
    for index in &subset {
        *layer_counts.entry(strata[*index]).or_insert(0usize) += 1;
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "pairs": pairs.len(),
            "group": group,
            "resamples": resamples,
            "baseline": {
                "spearman": point_base,
                "ci95": ci_base,
            },
            "quantized": {
                "spearman": point_quant,
                "ci95": ci_quant,
            },
            "delta": point_quant - point_base,
            "paired_p_value": p_value,
            "significant_at_0_05": p_value.map(|p| p < 0.05 || p > 0.95).unwrap_or(false),
            "stratified_subset": {
                "ratio": 0.4,
                "size": subset.len(),
                "stratum_counts": layer_counts,
                "baseline": subset_base,
                "quantized": subset_quant,
            },
        }))
        .expect("json")
    );
}
