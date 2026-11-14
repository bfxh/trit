//! `classifier_sts` 与 `classifier_sts_bert` 共用的分词与统计工具。
//!
//! 放在 `examples/sts/` 子目录里，Cargo 不会把它当作独立示例。

use std::collections::HashMap;

use trit_wspace::safetensors;

/// 一个句子的词向量序列。
pub type Sentence = Vec<Vec<f32>>;

pub const UNK: &str = "[UNK]";

pub fn load_vocab(path: &str) -> HashMap<String, usize> {
    std::fs::read_to_string(path)
        .expect("vocab.txt")
        .lines()
        .enumerate()
        .map(|(index, line)| (line.trim_end().to_string(), index))
        .collect()
}

/// BERT 的 `_is_punctuation`：ASCII 段落精确，非 ASCII 用"非字母数字且非空白"近似
/// （标准实现按 Unicode P* 类别判断，标准库没有该信息）。
pub fn is_punctuation(ch: char) -> bool {
    let code = ch as u32;
    if (33..=47).contains(&code)
        || (58..=64).contains(&code)
        || (91..=96).contains(&code)
        || (123..=126).contains(&code)
    {
        return true;
    }
    code > 127 && !ch.is_alphanumeric() && !ch.is_whitespace()
}

/// 标准 BasicTokenizer：清理控制字符、空白归一、小写、**标点切成独立 token（保留）**。
///
/// 早期版本把标点直接丢弃，与 BERT 不一致（`can't` 会被切成 `can`+`t` 而不是
/// `can`+`'`+`t`），实测这一步影响 STS 约 7 个点。CJK 字符分隔与去重音没有实现。
pub fn basic_tokens(text: &str) -> Vec<String> {
    let mut cleaned = String::with_capacity(text.len());
    for ch in text.chars() {
        let code = ch as u32;
        if code == 0 || code == 0xFFFD {
            continue;
        }
        if ch.is_whitespace() {
            cleaned.push(' ');
        } else {
            cleaned.push(ch);
        }
    }
    let mut tokens = Vec::new();
    for word in cleaned.split_whitespace() {
        let mut current = String::new();
        for ch in word.chars().flat_map(char::to_lowercase) {
            if is_punctuation(ch) {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(ch.to_string());
            } else {
                current.push(ch);
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
    }
    tokens
}

/// 标准 greedy longest-match WordPiece。
pub fn wordpiece(word: &str, vocab: &HashMap<String, usize>, out: &mut Vec<usize>) {
    if let Some(id) = vocab.get(word) {
        out.push(*id);
        return;
    }
    let chars: Vec<char> = word.chars().collect();
    let mut start = 0usize;
    let mut pieces = Vec::new();
    while start < chars.len() {
        let mut end = chars.len();
        let mut found: Option<(usize, usize)> = None;
        while start < end {
            let mut candidate: String = chars[start..end].iter().collect();
            if start > 0 {
                candidate.insert_str(0, "##");
            }
            if let Some(id) = vocab.get(&candidate) {
                found = Some((*id, end));
                break;
            }
            end -= 1;
        }
        match found {
            Some((id, next)) => {
                pieces.push(id);
                start = next;
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

pub fn encode(text: &str, vocab: &HashMap<String, usize>) -> Vec<usize> {
    let mut ids = Vec::new();
    for word in basic_tokens(text) {
        wordpiece(&word, vocab, &mut ids);
    }
    ids
}

/// BERT 的 GELU（tanh 近似）。
pub fn gelu(value: f32) -> f32 {
    let scale = (2.0f32 / std::f32::consts::PI).sqrt();
    0.5 * value * (1.0 + (scale * (value + 0.044_715 * value * value * value)).tanh())
}

pub fn mean(vectors: &[Vec<f32>], dim: usize) -> Vec<f32> {
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

pub fn cosine(left: &[f32], right: &[f32]) -> f64 {
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

/// 平均秩（并列取均值）。
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
pub fn spearman(left: &[f64], right: &[f64]) -> f64 {
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

pub fn read_pairs(path: &str, limit: usize) -> Vec<(String, String, f64)> {
    std::fs::read_to_string(path)
        .expect("tsv")
        .lines()
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

/// 读一个张量，`expect` 失败时带上名字。
pub fn tensor(path: &str, name: &str) -> safetensors::Weights {
    safetensors::read_tensor(path, name).unwrap_or_else(|error| panic!("{name}: {error:?}"))
}
