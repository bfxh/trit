//! 真实 STS 任务：块是否让表示更贴近人类相似度判断？
//!
//! 之前所有任务都是合成标签，本示例用**真实标注语料**：STS Benchmark（句子对 + 0–5 人类
//! 相似度分）。指标是余弦相似度与人类分数的 **Spearman 相关**（STS 标准指标）。
//!
//! 三个读数共用同一批 token 表示：
//! - `raw`：词向量（MiniLM word embedding）对 token 取平均；
//! - `layer 0`：MiniLM 第 0 层输出对 token 取平均；
//! - `layer 5`：第 5 层输出对 token 取平均（层间真实权重堆叠）。
//!
//! 词表示是简化实现（需在结论里一起读）：
//! - basic tokenizer + greedy longest-match WordPiece（标准算法，但没做小写以外的规范化）；
//! - **不加 [CLS]/[SEP]，也没有 position embedding**；
//! - 句向量取 token 平均（不是 CLS 池化）。
//!
//! 用法：`classifier_sts <model.safetensors> <vocab.txt> <test.tsv> [对数]`

use std::collections::HashMap;

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_cls::forward_with;

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const DEFAULT_PAIRS: usize = 500;
const UNK: &str = "[UNK]";

/// 一个句子的词向量序列。
type Sentence = Vec<Vec<f32>>;
/// 一对句子（左、右）的词向量序列。
type Pair = (Sentence, Sentence);

fn layer_key(layer: usize, suffix: &str) -> String {
    format!("encoder.layer.{layer}.{suffix}")
}

fn load_layer(path: &str, layer: usize, dim: usize) -> TransformerBlock {
    let query = safetensors::read_tensor(path, &layer_key(layer, "attention.self.query.weight"))
        .expect("query");
    let key = safetensors::read_tensor(path, &layer_key(layer, "attention.self.key.weight"))
        .expect("key");
    let value = safetensors::read_tensor(path, &layer_key(layer, "attention.self.value.weight"))
        .expect("value");
    let ln1_w =
        safetensors::read_vector(path, &layer_key(layer, "attention.output.LayerNorm.weight"))
            .expect("ln1 weight");
    let ln1_b =
        safetensors::read_vector(path, &layer_key(layer, "attention.output.LayerNorm.bias"))
            .expect("ln1 bias");
    let ff1_w = safetensors::read_tensor(path, &layer_key(layer, "intermediate.dense.weight"))
        .expect("ff1");
    let ff1_b = safetensors::read_vector(path, &layer_key(layer, "intermediate.dense.bias"))
        .expect("ff1 bias");
    let ff2_w =
        safetensors::read_tensor(path, &layer_key(layer, "output.dense.weight")).expect("ff2");
    let ff2_b =
        safetensors::read_vector(path, &layer_key(layer, "output.dense.bias")).expect("ff2 bias");
    let ln2_w =
        safetensors::read_vector(path, &layer_key(layer, "output.LayerNorm.weight")).expect("ln2w");
    let ln2_b =
        safetensors::read_vector(path, &layer_key(layer, "output.LayerNorm.bias")).expect("ln2b");
    TransformerBlock::new(
        F32Attention::new(query.values, key.values, value.values, dim, dim).expect("attention"),
        LayerNorm::new(ln1_w, ln1_b).expect("ln1"),
        Dense::new(ff1_w.values, ff1_b, ff1_w.rows, ff1_w.cols).expect("ff1"),
        Dense::new(ff2_w.values, ff2_b, ff2_w.rows, ff2_w.cols).expect("ff2"),
        LayerNorm::new(ln2_w, ln2_b).expect("ln2"),
    )
    .expect("block")
}

fn load_vocab(path: &str) -> HashMap<String, usize> {
    let text = std::fs::read_to_string(path).expect("vocab.txt");
    text.lines()
        .enumerate()
        .map(|(index, line)| (line.trim_end().to_string(), index))
        .collect()
}

/// basic tokenize：小写 + 按非字母数字切分（bert-uncased 的近似）。
fn basic_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        let lower = ch.to_lowercase().next().unwrap_or(ch);
        if lower.is_alphanumeric() {
            current.push(lower);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// 标准 greedy longest-match WordPiece。
fn wordpiece(word: &str, vocab: &HashMap<String, usize>, out: &mut Vec<usize>) {
    if let Some(id) = vocab.get(word) {
        out.push(*id);
        return;
    }
    let chars: Vec<char> = word.chars().collect();
    let mut start = 0usize;
    let mut pieces = Vec::new();
    while start < chars.len() {
        let mut end = chars.len();
        let mut found: Option<(String, usize)> = None;
        while start < end {
            let mut candidate: String = chars[start..end].iter().collect();
            if start > 0 {
                candidate.insert_str(0, "##");
            }
            if let Some(id) = vocab.get(&candidate) {
                found = Some((candidate, *id));
                break;
            }
            end -= 1;
        }
        match found {
            Some((_, id)) => {
                pieces.push(id);
                start = end;
            }
            None => {
                pieces.clear();
                break;
            }
        }
    }
    if pieces.is_empty() {
        out.push(*vocab.get(UNK).unwrap_or(&100));
    } else {
        out.extend(pieces);
    }
}

fn encode(text: &str, vocab: &HashMap<String, usize>) -> Vec<usize> {
    let mut ids = Vec::new();
    for word in basic_tokens(text) {
        wordpiece(&word, vocab, &mut ids);
    }
    ids
}

fn mean(vectors: &[Vec<f32>], dim: usize) -> Vec<f32> {
    let mut pooled = vec![0.0f32; dim];
    if vectors.is_empty() {
        return pooled;
    }
    for vector in vectors {
        for (slot, value) in pooled.iter_mut().zip(vector) {
            *slot += value;
        }
    }
    let count = vectors.len() as f32;
    for slot in pooled.iter_mut() {
        *slot /= count;
    }
    pooled
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    let dot: f64 = left
        .iter()
        .zip(right)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    let ln: f64 = left
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    let rn: f64 = right
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    if ln == 0.0 || rn == 0.0 {
        0.0
    } else {
        dot / (ln * rn)
    }
}

/// 平均秩（并列取均值），返回排名序列。
fn ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|left, right| values[*left].partial_cmp(&values[*right]).unwrap());
    let mut result = vec![0.0f64; values.len()];
    let mut index = 0usize;
    while index < order.len() {
        let mut end = index + 1;
        while end < order.len() && (values[order[end]] - values[order[index]]).abs() < 1e-12 {
            end += 1;
        }
        let average = (index + end - 1) as f64 / 2.0 + 1.0;
        for slot in index..end {
            result[order[slot]] = average;
        }
        index = end;
    }
    result
}

/// Spearman = 秩上的 Pearson。
fn spearman(left: &[f64], right: &[f64]) -> f64 {
    let lr = ranks(left);
    let rr = ranks(right);
    let n = lr.len() as f64;
    let mean_l: f64 = lr.iter().sum::<f64>() / n;
    let mean_r: f64 = rr.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut var_l = 0.0;
    let mut var_r = 0.0;
    for (a, b) in lr.iter().zip(&rr) {
        let da = a - mean_l;
        let db = b - mean_r;
        cov += da * db;
        var_l += da * da;
        var_r += db * db;
    }
    if var_l == 0.0 || var_r == 0.0 {
        0.0
    } else {
        cov / (var_l.sqrt() * var_r.sqrt())
    }
}

fn read_pairs(path: &str, limit: usize) -> Vec<(String, String, f64)> {
    let text = std::fs::read_to_string(path).expect("tsv");
    text.lines()
        .take(limit)
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let a = parts.next()?.to_string();
            let b = parts.next()?.to_string();
            let score: f64 = parts.next()?.trim().parse().ok()?;
            Some((a, b, score))
        })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .expect("用法: classifier_sts <model> <vocab> <tsv> [对数]");
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
    let dim = embeddings.cols;
    let mut unknown = 0usize;
    let mut total_tokens = 0usize;

    // 把每对句子先编码成词向量序列（三个读数共用）。
    let mut encoded: Vec<Pair> = Vec::with_capacity(pairs.len());
    for (left, right, _) in &pairs {
        let mut pair = Vec::with_capacity(2);
        for sentence in [left, right] {
            let ids = encode(sentence, &vocab);
            total_tokens += ids.len();
            unknown += ids
                .iter()
                .filter(|id| **id == *vocab.get(UNK).unwrap_or(&100))
                .count();
            let vectors: Vec<Vec<f32>> = ids
                .iter()
                .map(|id| embeddings.values[id * dim..(id + 1) * dim].to_vec())
                .collect();
            pair.push(if vectors.is_empty() {
                vec![vec![0.0f32; dim]]
            } else {
                vectors
            });
        }
        encoded.push((pair[0].clone(), pair[1].clone()));
    }

    let gold: Vec<f64> = pairs.iter().map(|(_, _, score)| *score).collect();

    let mut readings = Vec::new();
    // raw：词向量平均。
    {
        let predicted: Vec<f64> = encoded
            .iter()
            .map(|(a, b)| cosine(&mean(a, dim), &mean(b, dim)))
            .collect();
        readings.push(json!({ "readout": "raw", "spearman": spearman(&predicted, &gold) }));
    }

    // 逐层：句内 self-attention 后取平均。
    for layer in [0usize, 5] {
        let block = load_layer(&model, layer, dim);
        let predicted: Vec<f64> = encoded
            .iter()
            .map(|(a, b)| {
                let left = forward_with(&block, a, a).expect("left forward");
                let right = forward_with(&block, b, b).expect("right forward");
                cosine(&mean(&left, dim), &mean(&right, dim))
            })
            .collect();
        readings.push(json!({
            "readout": format!("layer {layer}"),
            "spearman": spearman(&predicted, &gold),
        }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "pairs": pairs.len(),
            "dim": dim,
            "vocab": vocab.len(),
            "unk_rate": unknown as f64 / total_tokens.max(1) as f64,
            "mean_tokens": total_tokens as f64 / (pairs.len() * 2).max(1) as f64,
            "metric": "spearman(cosine, gold score)",
            "readings": readings,
        }))
        .expect("json")
    );
}
