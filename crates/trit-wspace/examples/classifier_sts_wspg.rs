//! 闭环验证：把 MiniLM 的线性权重**真正落盘成 WSP1G**，读回，再测 STS。
//!
//! 这一路测过的结论是分散的：
//! - 分组三值在 STS 上几乎无损（−0.0004 点，全量 1379 对）；
//! - 但文本格式实测压缩率只有 1.13×（`grouped_codec`）；
//! - 且判等不再能忽略 scale（`grouped_equal`）。
//!
//! 本示例把它们串成一条可执行链路：
//!
//! ```text
//! f32 权重 → GroupedLayer::quantize → to_wspg（写文件）→ from_wspg（读回）
//!        → to_f32 → 装回 BERT 层 → 跑 STS
//! ```
//!
//! 三个读数应当一致或有可解释的差异：
//! 1. `baseline`：原始 f32；
//! 2. `quantized`：内存里量化后（未序列化）；
//! 3. `roundtrip`：写成 WSP1G 文本、读回、再装回。
//!
//! **`quantized` 与 `roundtrip` 必须逐位一致**——这正是"存→读→行为不变"。
//!
//! 用法：`classifier_sts_wspg <model> <vocab> <test.tsv> [对数] [group]`

use std::collections::HashMap;

use serde_json::json;
use trit_wspace::grouped::GroupedLayer;
use trit_wspace::grouped_codec::{from_wspg, to_wspg};
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::{Dense, LayerNorm};

#[path = "sts/bert.rs"]
#[allow(dead_code)]
mod bert;
#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use bert::BertLayer;
use common::{Sentence, cosine, encode, load_vocab, mean, read_pairs, spearman};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";

/// 一层里要量化的 6 个线性权重（名字后缀）。
const LINEAR: [&str; 6] = [
    "attention.self.query.weight",
    "attention.self.key.weight",
    "attention.self.value.weight",
    "attention.output.dense.weight",
    "intermediate.dense.weight",
    "output.dense.weight",
];

/// 把一层里 6 个线性权重量化、序列化、读回，并替换成读回后的等效 f32。
///
/// 返回 `(写盘字节数, 读回与原量化层是否逐位一致)`。
fn roundtrip_layer(
    layer: &mut BertLayer,
    model: &str,
    index: usize,
    group: usize,
    dir: &str,
) -> (usize, bool) {
    let mut bytes = 0usize;
    let mut all_equal = true;
    for (slot, suffix) in LINEAR.iter().enumerate() {
        let name = format!("encoder.layer.{index}.{suffix}");
        let weight = safetensors::read_tensor(model, &name).expect("weight");
        let bias =
            safetensors::read_vector(model, &name.replace(".weight", ".bias")).expect("bias");
        let quantized =
            GroupedLayer::quantize(weight.rows, weight.cols, &weight.values, &bias, group)
                .expect("quantize");

        // 真正写盘再读回（不是内存里的往返）
        let text = to_wspg(&quantized);
        bytes += text.len();
        let path = format!("{dir}/layer{index}_{slot}.wsp1g");
        std::fs::write(&path, text.as_bytes()).expect("write");
        let loaded = std::fs::read_to_string(&path).expect("read");
        let restored = from_wspg(&loaded).expect("parse");

        // 关键断言：读回后必须与量化层逐位一致
        if restored.to_f32() != quantized.to_f32() {
            all_equal = false;
        }

        // 装回 BERT 层（用读回后的等效权重 + 原 bias）
        let values = restored.to_f32();
        let replacement = Dense::new(values, bias, weight.rows, weight.cols).expect("dense");
        match *suffix {
            "attention.self.query.weight" => layer.wq = replacement,
            "attention.self.key.weight" => layer.wk = replacement,
            "attention.self.value.weight" => layer.wv = replacement,
            "attention.output.dense.weight" => layer.wo = replacement,
            "intermediate.dense.weight" => layer.ff1 = replacement,
            _ => layer.ff2 = replacement,
        }
    }
    (bytes, all_equal)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts_wspg <model> <vocab> <test.tsv> [对数] [group]");
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

    // 1) 基线（原始 f32）
    let baseline_layers: Vec<BertLayer> = (0..6).map(|i| BertLayer::load(&model, i)).collect();
    let baseline = evaluate(&baseline_layers);

    // 2) 量化后（内存，未序列化）
    let mut quantized_layers: Vec<BertLayer> = (0..6).map(|i| BertLayer::load(&model, i)).collect();
    let mut memory_bytes = 0usize;
    for (index, layer) in quantized_layers.iter_mut().enumerate() {
        for suffix in LINEAR {
            let name = format!("encoder.layer.{index}.{suffix}");
            let weight = safetensors::read_tensor(&model, &name).expect("weight");
            let bias =
                safetensors::read_vector(&model, &name.replace(".weight", ".bias")).expect("bias");
            let quantized =
                GroupedLayer::quantize(weight.rows, weight.cols, &weight.values, &bias, group)
                    .expect("quantize");
            memory_bytes += to_wspg(&quantized).len();
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
    let quantized = evaluate(&quantized_layers);

    // 3) 真正写盘、读回、再装回
    let dir = std::env::temp_dir().join("wspg-layers");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let dir = dir.to_string_lossy().to_string();
    let mut roundtrip_layers: Vec<BertLayer> = (0..6).map(|i| BertLayer::load(&model, i)).collect();
    let mut disk_bytes = 0usize;
    let mut all_equal = true;
    for (index, layer) in roundtrip_layers.iter_mut().enumerate() {
        let (bytes, equal) = roundtrip_layer(layer, &model, index, group, &dir);
        disk_bytes += bytes;
        all_equal &= equal;
    }
    let roundtrip = evaluate(&roundtrip_layers);

    // f32 原始字节数（只算被量化的 6×6 个权重）
    let mut f32_bytes = 0usize;
    for index in 0..6 {
        for suffix in LINEAR {
            let name = format!("encoder.layer.{index}.{suffix}");
            let weight = safetensors::read_tensor(&model, &name).expect("weight");
            f32_bytes += weight.rows * weight.cols * 4;
        }
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "pairs": pairs.len(),
            "group": group,
            "spearman": {
                "baseline": baseline,
                "quantized": quantized,
                "roundtrip": roundtrip,
            },
            "delta": {
                "quantized_vs_baseline": quantized - baseline,
                "roundtrip_vs_quantized": roundtrip - quantized,
            },
            "roundtrip_bit_identical": all_equal,
            "bytes": {
                "f32": f32_bytes,
                "wsp1g_in_memory": memory_bytes,
                "wsp1g_on_disk": disk_bytes,
                "compression_vs_f32": f32_bytes as f64 / disk_bytes.max(1) as f64,
            },
        }))
        .expect("json")
    );
}
