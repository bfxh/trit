//! STS 任务头：mean pooling + 句向量余弦，对人工分算 Pearson / Spearman。

use std::fs;

use serde_json::{Value, json};

pub(crate) struct Pair {
    pub(crate) a: usize,
    pub(crate) b: usize,
    pub(crate) score: f32,
}

pub(crate) fn load_pairs(data_dir: &str) -> Vec<Pair> {
    let path = format!("{data_dir}/pairs.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    value["pairs"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(Pair {
                        a: item["a"].as_u64()? as usize,
                        b: item["b"].as_u64()? as usize,
                        score: item["score"].as_f64()? as f32,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 每个句子按 mask 做 mean pooling，再 L2 normalize。
pub(crate) fn sentence_vectors(
    hidden: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    width: usize,
) -> Vec<Vec<f32>> {
    let mut out = Vec::with_capacity(batch);
    for item in 0..batch {
        let mut sum = vec![0.0f64; width];
        let mut count = 0usize;
        for token in 0..seq {
            if mask.get(item * seq + token).is_none_or(|&bit| bit == 0) {
                continue;
            }
            let base = (item * seq + token) * width;
            for (index, value) in sum.iter_mut().enumerate() {
                *value += f64::from(hidden[base + index]);
            }
            count += 1;
        }
        if count > 0 {
            for value in &mut sum {
                *value /= count as f64;
            }
        }
        let norm = sum.iter().map(|value| value * value).sum::<f64>().sqrt();
        if norm > 0.0 {
            for value in &mut sum {
                *value /= norm;
            }
        }
        out.push(sum.iter().map(|value| *value as f32).collect());
    }
    out
}

pub(crate) fn metrics(vectors: &[Vec<f32>], pairs: &[Pair]) -> Value {
    let mut estimated = Vec::with_capacity(pairs.len());
    let mut gold = Vec::with_capacity(pairs.len());
    for pair in pairs {
        if pair.a >= vectors.len() || pair.b >= vectors.len() {
            continue;
        }
        let a = &vectors[pair.a];
        let b = &vectors[pair.b];
        let dot = a
            .iter()
            .zip(b.iter())
            .map(|(left, right)| f64::from(*left) * f64::from(*right))
            .sum::<f64>();
        estimated.push(dot);
        gold.push(f64::from(pair.score));
    }
    let pearson_r = pearson(&estimated, &gold);
    let spearman_r = pearson(&ranks(&estimated), &ranks(&gold));
    json!({
        "pairs": estimated.len(),
        "pearson": pearson_r,
        "spearman": spearman_r,
    })
}

fn pearson(left: &[f64], right: &[f64]) -> f64 {
    if left.len() < 2 || left.len() != right.len() {
        return 0.0;
    }
    let n = left.len() as f64;
    let mean_left = left.iter().sum::<f64>() / n;
    let mean_right = right.iter().sum::<f64>() / n;
    let mut covariance = 0.0;
    let mut var_left = 0.0;
    let mut var_right = 0.0;
    for (a, b) in left.iter().zip(right) {
        let da = a - mean_left;
        let db = b - mean_right;
        covariance += da * db;
        var_left += da * da;
        var_right += db * db;
    }
    let denominator = (var_left * var_right).sqrt();
    if denominator > 0.0 {
        covariance / denominator
    } else {
        0.0
    }
}

fn ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| {
        values[a]
            .partial_cmp(&values[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = vec![0.0; values.len()];
    let mut index = 0;
    while index < order.len() {
        let mut end = index;
        while end + 1 < order.len() && (values[order[end + 1]] - values[order[index]]).abs() < 1e-12
        {
            end += 1;
        }
        let rank = (index + end) as f64 / 2.0 + 1.0;
        for slot in index..=end {
            out[order[slot]] = rank;
        }
        index = end + 1;
    }
    out
}
