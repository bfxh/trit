//! 报告工具：误差汇总、输入分布统计、随机输入生成。

use serde_json::{Value, json};
use trit_wspace::activations;

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

/// 逐层 f32 输出与导出的 `layerN.f32` 对拍；缺文件就停。
pub(crate) fn layer_refs(
    data_dir: &str,
    layers: &[Vec<f32>],
    mask: &[u8],
    hidden: usize,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (index, values) in layers.iter().enumerate() {
        let path = format!("{data_dir}/layer{index}.f32");
        match activations::read_matrix(&path) {
            Ok((rows, cols, reference)) => {
                assert_eq!(rows * cols, values.len(), "layer{index} 形状不符");
                out.push(compare(values, &reference, mask, hidden));
            }
            Err(_) => break,
        }
    }
    out
}
