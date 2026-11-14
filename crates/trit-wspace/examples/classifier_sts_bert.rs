//! 真实 STS + **完整 BERT 层**：CLS/位置嵌入 + 注意力输出投影 + 6 层堆叠。
//!
//! 上一个版本（`classifier_sts` 的 bert 模式）暴露了实现的不足：`TransformerBlock`
//! 只有 attention + LN + FFN，**缺 BERT 的 `attention.output.dense`**，也忽略全部
//! attention bias。单层近似还过得去（U 形趋势可信），堆叠 6 层就崩（CLS 池化 Spearman
//! 掉到 0.03）。本示例在示例内实现**完整的 BERT 层**，用来验证：
//!
//! 1. 补齐后 CLS 池化能否恢复（真实 MiniLM 在 STS 上约 0.82）；
//! 2. 6 层堆叠是否优于单层；
//! 3. token 平均 vs CLS 池化哪个更好。
//!
//! 用法：`classifier_sts_bert <model> <vocab.txt> <test.tsv> [对数]`

use serde_json::json;
use trit_wspace::safetensors;
#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use common::{Sentence, cosine, encode, gelu, load_vocab, mean, read_pairs, spearman};
use trit_wspace::transformer_f32::{Dense, LayerNorm};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";
const CLS_ID: usize = 101;
const SEP_ID: usize = 102;
const DEFAULT_PAIRS: usize = 1379;
const HEADS: usize = 12;

fn layer_key(layer: usize, suffix: &str) -> String {
    format!("encoder.layer.{layer}.{suffix}")
}

fn dense(path: &str, name: &str) -> Dense {
    let weight = safetensors::read_tensor(path, name).expect("dense weight");
    let bias = safetensors::read_vector(path, &name.replace(".weight", ".bias")).expect("bias");
    Dense::new(weight.values, bias, weight.rows, weight.cols).expect("dense")
}

fn layernorm(path: &str, name: &str) -> LayerNorm {
    let weight = safetensors::read_vector(path, &name.replace(".bias", ".weight")).expect("ln w");
    let bias = safetensors::read_vector(path, name).expect("ln b");
    LayerNorm::new(weight, bias).expect("ln")
}

/// 完整的一层 BERT（post-LN）：
/// `h = LN1(x + Wo·attn)`，`h = LN2(h + FF2(act(FF1(h))))`。
struct BertLayer {
    wq: Dense,
    wk: Dense,
    wv: Dense,
    wo: Dense,
    ln1: LayerNorm,
    ff1: Dense,
    ff2: Dense,
    ln2: LayerNorm,
}

impl BertLayer {
    fn load(path: &str, layer: usize) -> Self {
        Self {
            wq: dense(path, &layer_key(layer, "attention.self.query.weight")),
            wk: dense(path, &layer_key(layer, "attention.self.key.weight")),
            wv: dense(path, &layer_key(layer, "attention.self.value.weight")),
            wo: dense(path, &layer_key(layer, "attention.output.dense.weight")),
            ln1: layernorm(path, &layer_key(layer, "attention.output.LayerNorm.bias")),
            ff1: dense(path, &layer_key(layer, "intermediate.dense.weight")),
            ff2: dense(path, &layer_key(layer, "output.dense.weight")),
            ln2: layernorm(path, &layer_key(layer, "output.LayerNorm.bias")),
        }
    }

    fn forward(&self, sequence: &Sentence) -> Sentence {
        let dim = sequence[0].len();
        let head_dim = dim / HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let keys: Vec<Vec<f32>> = sequence
            .iter()
            .map(|token| self.wk.forward(token, false))
            .collect();
        let values: Vec<Vec<f32>> = sequence
            .iter()
            .map(|token| self.wv.forward(token, false))
            .collect();
        let mut out = Vec::with_capacity(sequence.len());
        for token in sequence {
            let query = self.wq.forward(token, false);
            let mut pooled = vec![0.0f32; dim];
            // 真多头：每个头 32 维各自打分，拼回 384 维再乘 output projection。
            for head in 0..HEADS {
                let offset = head * head_dim;
                let span = offset..offset + head_dim;
                let query_head = &query[span.clone()];
                let scores: Vec<f32> = keys
                    .iter()
                    .map(|key| {
                        query_head
                            .iter()
                            .zip(&key[span.clone()])
                            .map(|(a, b)| a * b)
                            .sum::<f32>()
                            * scale
                    })
                    .collect();
                let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let exps: Vec<f32> = scores.iter().map(|score| (score - max).exp()).collect();
                let total: f32 = exps.iter().sum();
                for (weight, value) in exps.iter().zip(&values) {
                    let w = *weight / total;
                    for index in span.clone() {
                        pooled[index] += w * value[index];
                    }
                }
            }
            let projected = self.wo.forward(&pooled, false);
            let residual: Vec<f32> = token.iter().zip(&projected).map(|(a, b)| a + b).collect();
            let hidden = self.ln1.forward(&residual);
            // BERT 用 GELU；`Dense::forward(x, true)` 是 ReLU。
            let pre = self.ff1.forward(&hidden, false);
            let mid: Vec<f32> = pre.iter().map(|value| gelu(*value)).collect();
            let ffn = self.ff2.forward(&mid, false);
            let residual2: Vec<f32> = hidden.iter().zip(&ffn).map(|(a, b)| a + b).collect();
            out.push(self.ln2.forward(&residual2));
        }
        out
    }
}

/// `[CLS] + tokens + [SEP]`，词向量 + position + token_type，再过 embeddings LayerNorm。
fn bert_sequence(
    ids: &[usize],
    embeddings: &safetensors::Weights,
    position: &safetensors::Weights,
    token_type: &safetensors::Weights,
    emb_ln: &LayerNorm,
    dim: usize,
) -> Sentence {
    let mut full = Vec::with_capacity(ids.len() + 2);
    full.push(CLS_ID);
    full.extend_from_slice(ids);
    full.push(SEP_ID);
    full.iter()
        .enumerate()
        .map(|(slot, id)| {
            let mut vector = embeddings.values[id * dim..(id + 1) * dim].to_vec();
            // token_type 段号恒为 0，但该段的嵌入向量本身非零，必须加上。
            let segment = &token_type.values[..dim];
            for (index, value) in vector.iter_mut().enumerate() {
                *value += segment[index];
            }
            if slot < position.rows {
                let offset = slot * dim;
                for (index, value) in vector.iter_mut().enumerate() {
                    *value += position.values[offset + index];
                }
            }
            emb_ln.forward(&vector)
        })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts_bert <model> <vocab> <tsv> [对数]");
    let vocab_path = args.next().expect("vocab.txt");
    let tsv_path = args.next().expect("test.tsv");
    let pairs = read_pairs(
        &tsv_path,
        args.next()
            .map(|value| value.parse().expect("pairs"))
            .unwrap_or(DEFAULT_PAIRS),
    );

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
    let layers: Vec<BertLayer> = (0..6).map(|layer| BertLayer::load(&model, layer)).collect();

    let gold: Vec<f64> = pairs.iter().map(|(_, _, score)| *score).collect();
    let mut cls_predicted = Vec::with_capacity(pairs.len());
    let mut mean_predicted = Vec::with_capacity(pairs.len());

    for (left, right, _) in &pairs {
        let mut pooled = Vec::with_capacity(2);
        for sentence in [left, right] {
            let ids = encode(sentence, &vocab);
            let mut sequence =
                bert_sequence(&ids, &embeddings, &position, &token_type, &emb_ln, dim);
            for layer in &layers {
                sequence = layer.forward(&sequence);
            }
            // 模型配置是 mean pooling（pooling_mode_mean_tokens=true、cls=false），
            // 所以主读数取整句平均（含 [CLS]/[SEP]，与 sentence-transformers 默认一致）。
            let cls = sequence[0].clone();
            let all = mean(&sequence, dim);
            pooled.push((cls, all));
        }
        cls_predicted.push(cosine(&pooled[0].0, &pooled[1].0));
        mean_predicted.push(cosine(&pooled[0].1, &pooled[1].1));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode": "full BERT layer (QKV bias + output projection + post-LN)",
            "layers": layers.len(),
            "pairs": pairs.len(),
            "dim": dim,
            "metric": "spearman(cosine, gold score)",
            "readings": [
                { "readout": "stack6 + CLS pooling (对照，模型不采用)", "spearman": spearman(&cls_predicted, &gold) },
                { "readout": "stack6 + mean pooling (模型配置)", "spearman": spearman(&mean_predicted, &gold) },
            ],
        }))
        .expect("json")
    );
}
