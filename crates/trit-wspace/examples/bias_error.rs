//! bias 量化误差读数：f32 权重+bias 作为精确参照，比较三值权重 + {0, sign(b)} bias。
//!
//! 用法：
//! ```bash
//! cargo run --release -p trit-wspace --example bias_error -- \
//!   --file model.safetensors --tensor dense.weight --bias dense.bias --inputs 256
//! ```
//! 不带 `--file` 时用合成张量（32×64），便于本地快速复算。

use serde_json::json;
use trit_wspace::safetensors::{self, Weights};

struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 2001) as f32 / 1000.0 - 1.0
    }
}

fn synthetic(bias_scale: f32) -> (usize, usize, Vec<f32>, Vec<f32>) {
    let (rows, cols) = (32usize, 64usize);
    let mut rng = Rng(0x5eed_2026);
    let weights = (0..rows * cols).map(|_| rng.next_f32() * 0.1).collect();
    let bias = (0..rows).map(|_| rng.next_f32() * bias_scale).collect();
    (rows, cols, weights, bias)
}

fn main() {
    let mut file = String::new();
    let mut tensor = String::new();
    let mut bias_name = None;
    let mut inputs = 256usize;
    let mut seed = 7u64;
    let mut bias_scale = 0.05f32;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--file" => file = args.next().expect("file"),
            "--tensor" => tensor = args.next().expect("tensor"),
            "--bias" => bias_name = Some(args.next().expect("bias")),
            "--inputs" => inputs = args.next().expect("inputs").parse().expect("num"),
            "--seed" => seed = args.next().expect("seed").parse().expect("num"),
            "--bias-scale" => bias_scale = args.next().expect("bias-scale").parse().expect("num"),
            other => panic!("unknown arg {other}"),
        }
    }
    let (rows, cols, weights, bias_values) = if file.is_empty() {
        synthetic(bias_scale)
    } else {
        let weights = safetensors::read_tensor(&file, &tensor).expect("weight");
        let bias = match &bias_name {
            Some(name) => safetensors::read_vector(&file, name).expect("bias"),
            None => vec![0.0; weights.rows],
        };
        (weights.rows, weights.cols, weights.values, bias)
    };
    let layer = Weights {
        rows,
        cols,
        values: weights.clone(),
    }
    .to_layer_with_bias(Some(&bias_values))
    .expect("layer");
    let mut rng = Rng(seed | 1);
    let mut mse_zero = 0.0f64;
    let mut mse_sign = 0.0f64;
    let mut count = 0usize;
    for _ in 0..inputs {
        let input: Vec<f32> = (0..cols).map(|_| rng.next_f32()).collect();
        for row in 0..rows {
            let mut exact = f64::from(bias_values[row]);
            let mut quantized = 0.0f64;
            for col in 0..cols {
                exact += f64::from(weights[row * cols + col]) * f64::from(input[col]);
                let trit = layer.trit(row, col).expect("trit").to_i8();
                quantized += f64::from(trit) * f64::from(input[col]);
            }
            let scale = f64::from(layer.scale(row).expect("scale"));
            let sign = f64::from(layer.bias_trit(row).expect("bias").to_i8());
            let zero = scale * quantized;
            let signed = scale * (quantized + sign);
            mse_zero += (zero - exact).powi(2);
            mse_sign += (signed - exact).powi(2);
            count += 1;
        }
    }
    mse_zero /= count as f64;
    mse_sign /= count as f64;
    let mean_abs_bias = bias_values
        .iter()
        .map(|value| f64::from(value.abs()))
        .sum::<f64>()
        / rows as f64;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "rows": rows,
            "cols": cols,
            "inputs": inputs,
            "mean_abs_bias": mean_abs_bias,
            "nonzero_bias": bias_values.iter().filter(|value| **value != 0.0).count(),
            "mse_zero_bias": mse_zero,
            "mse_sign_bias": mse_sign,
            "sign_over_zero": if mse_zero > 0.0 { mse_sign / mse_zero } else { 1.0 },
        }))
        .expect("json")
    );
}
