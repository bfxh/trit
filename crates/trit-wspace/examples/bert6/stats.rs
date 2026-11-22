//! 输入分布统计与随机输入（从 `report.rs` 拆出，守住行数预算）。

use serde_json::json;

pub(crate) fn input_stats(values: &[f32]) -> serde_json::Value {
    let n = values.len() as f64;
    let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / n;
    let mut m2 = 0.0;
    let mut m3 = 0.0;
    let mut m4 = 0.0;
    let mut max_abs = 0.0f64;
    let mut zeros = 0usize;
    for &value in values {
        let x = f64::from(value);
        let d = x - mean;
        m2 += d * d;
        m3 += d * d * d;
        m4 += d * d * d * d;
        max_abs = max_abs.max(x.abs());
        if value == 0.0 {
            zeros += 1;
        }
    }
    let var = m2 / n;
    let std = var.sqrt();
    let kurtosis = if var > 0.0 {
        (m4 / n) / (var * var) - 3.0
    } else {
        0.0
    };
    json!({
        "mean": mean,
        "std": std,
        "max_abs": max_abs,
        "skew": if var > 0.0 { (m3 / n) / var.powf(1.5) } else { 0.0 },
        "kurtosis_excess": kurtosis,
        "zero_frac": zeros as f64 / n,
        "norm": (m2 + n * mean * mean).sqrt(),
    })
}

pub(crate) fn random_like(values: &[f32], seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    values
        .iter()
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state % 2001) as f32 / 1000.0) - 1.0
        })
        .collect()
}
