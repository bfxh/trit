//! Pearson / Spearman 与可选的逐对预测导出（`TRIT_DUMP_PREDS=1`）。

use super::Pair;
use serde_json::{Value, json};

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
    let dump = std::env::var("TRIT_DUMP_PREDS")
        .ok()
        .is_some_and(|v| v.trim() == "1");
    let mut report = json!({
        "pairs": estimated.len(),
        "pearson": pearson_r,
        "spearman": spearman_r,
    });
    if dump {
        report["preds"] = json!(estimated);
    }
    report
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
