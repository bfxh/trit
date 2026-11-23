//! STS 任务头：mean pooling + 句向量余弦，对人工分算 Pearson / Spearman。

use std::fs;

use serde_json::Value;
mod metrics;
pub(crate) use metrics::metrics;

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
