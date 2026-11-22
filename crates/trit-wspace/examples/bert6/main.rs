//! MiniLM 完整 6 层前向：f32 参照 vs 三值线性权重（LN/GELU/softmax/残差保持 f32）。
//!
//! 数据由 `export_bert6.py` 导出（`WSPACT1` 矩阵 + `WSPMSK1` 掩码）：
//! - `input.f32`：第 0 层输入 hidden states（batch×seq×hidden）
//! - `mask.u8`：attention mask（batch×seq）
//! - `reference.f32`：PyTorch f32 最后一层输出，作为独立参照
//!
//! ```bash
//! cargo run --release -p trit-wspace --example bert6 -- \
//!   --file model.safetensors --data-dir "$TRIT_DATA/miniLM-bert6" --out report.json
//! ```

mod model;

use serde_json::{Value, json};
use trit_wspace::activations;

use model::Bert;

struct Acc {
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

fn compare(output: &[f32], reference: &[f32], mask: &[u8], hidden: usize) -> Value {
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

fn input_stats(values: &[f32]) -> Value {
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

fn random_like(values: &[f32], seed: u64) -> Vec<f32> {
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

fn main() {
    let mut file = String::new();
    let mut data_dir = String::new();
    let mut out_path = None;
    let mut seed = 7u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--file" => file = args.next().expect("file"),
            "--data-dir" => data_dir = args.next().expect("data-dir"),
            "--out" => out_path = Some(args.next().expect("out")),
            "--seed" => seed = args.next().expect("seed").parse().expect("num"),
            other => panic!("unknown arg {other}"),
        }
    }
    assert!(
        !file.is_empty() && !data_dir.is_empty(),
        "--file/--data-dir 必填"
    );

    let (batch, seq, mask) = activations::read_mask(format!("{data_dir}/mask.u8")).expect("mask");
    let (input_rows, hidden, input) =
        activations::read_matrix(format!("{data_dir}/input.f32")).expect("input");
    let (ref_rows, ref_cols, reference) =
        activations::read_matrix(format!("{data_dir}/reference.f32")).expect("reference");
    assert_eq!(input_rows, batch * seq);
    assert_eq!(ref_rows, batch * seq);
    assert_eq!(ref_cols, hidden);

    let heads = 12usize;
    let eps = 1e-12f32;
    let f32_bert = Bert::load(&file, 6, hidden, hidden * 4, heads, eps, false);
    let quant_bert = Bert::load(&file, 6, hidden, hidden * 4, heads, eps, true);

    let (f32_out, _, f32_layers) = f32_bert.forward(&input, &mask, batch, seq, false);
    let (quant_out, real_stages, _) = quant_bert.forward(&input, &mask, batch, seq, true);

    let random_input = random_like(&input, seed);
    let (f32_random, _, _) = f32_bert.forward(&random_input, &mask, batch, seq, false);
    let (quant_random, random_stages, _) =
        quant_bert.forward(&random_input, &mask, batch, seq, true);

    let mut layer_refs = Vec::new();
    for (index, values) in f32_layers.iter().enumerate() {
        let path = format!("{data_dir}/layer{index}.f32");
        match activations::read_matrix(&path) {
            Ok((rows, cols, reference_layer)) => {
                assert_eq!(rows * cols, values.len(), "layer{index} 形状不符");
                layer_refs.push(compare(values, &reference_layer, &mask, hidden));
            }
            Err(_) => break,
        }
    }

    let report = json!({
        "source": {
            "file": file,
            "data_dir": data_dir,
            "batch": batch,
            "seq": seq,
            "hidden": hidden,
            "heads": heads,
            "layers": 6,
            "quantized": "all linear weights (Q/K/V/O + FFN), f32 LN/GELU/softmax/residual",
        },
        "f32_vs_reference": compare(&f32_out, &reference, &mask, hidden),
        "f32_layers_vs_reference": layer_refs,
        "ternary_vs_reference": compare(&quant_out, &reference, &mask, hidden),
        "real_ternary_vs_f32": compare(&quant_out, &f32_out, &mask, hidden),
        "random_ternary_vs_f32": compare(&quant_random, &f32_random, &mask, hidden),
        "output_stage": {
            "real": real_stages.iter().map(|s| input_stats(s)).collect::<Vec<_>>(),
            "random": random_stages.iter().map(|s| input_stats(s)).collect::<Vec<_>>(),
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    match out_path {
        Some(path) => {
            std::fs::write(&path, text + "\n").expect("write");
            println!("写入 {path}");
        }
        None => println!("{text}"),
    }
}
