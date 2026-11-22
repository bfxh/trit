//! 报告工具：误差汇总、输入分布统计、随机输入生成。

use serde_json::{Value, json};

pub(crate) struct Acc {
    sq_err: f64,
    ref_sq: f64,
    out_sq: f64,
    dot: f64,
    count: u64,
}

impl Acc {
    fn add(&mut self, output: &[f32], reference: &[f32], mask: &[u8], hidden: usize) {
        for (index, (&out, &want)) in output.iter().zip(reference).enumerate() {
            if mask.get(index / hidden).is_some_and(|&bit| bit == 0) {
                continue;
            }
            let out = f64::from(out);
            let want = f64::from(want);
            let diff = out - want;
            self.sq_err += diff * diff;
            self.ref_sq += want * want;
            self.out_sq += out * out;
            self.dot += out * want;
            self.count += 1;
        }
    }

    fn finish(self) -> Value {
        let mse = if self.count == 0 {
            0.0
        } else {
            self.sq_err / self.count as f64
        };
        let rel_l2 = if self.ref_sq > 0.0 {
            (self.sq_err / self.ref_sq).sqrt()
        } else {
            0.0
        };
        let denominator = (self.ref_sq * self.out_sq).sqrt();
        json!({
            "mse": mse,
            "rel_l2": rel_l2,
            "cosine": if denominator > 0.0 { self.dot / denominator } else { 1.0 },
            "count": self.count,
        })
    }
}

pub(crate) fn compare(output: &[f32], reference: &[f32], mask: &[u8], hidden: usize) -> Value {
    let mut acc = Acc {
        sq_err: 0.0,
        ref_sq: 0.0,
        out_sq: 0.0,
        dot: 0.0,
        count: 0,
    };
    acc.add(output, reference, mask, hidden);
    acc.finish()
}

pub(crate) fn input_stats(values: &[f32]) -> Value {
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
