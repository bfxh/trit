//! 真实模型上的端到端 bias 读数：f32 权重作参照，比较三值权重 + {零 / 三值 / f32} bias。
//!
//! 读数分三组：逐层（每层独立，随机输入）、端到端级联（三层全部三值化）、
//! 单层替换（只把第 i 层三值化，其余层保持 f32）。每组都对比 f32 参照输出，
//! 报告 MSE / RMSE / 相对 L2 / 全局余弦。
//!
//! 用法：
//! ```bash
//! cargo run --release -p trit-wspace --example bias_e2e -- \
//!   --file model.safetensors --layer 0 --layers 6 --inputs 256
//! ```
//! 不带 `--file` 时用合成三层（64→128→64），便于快速复算。
//!
//! 边界：输入是 `uniform(-1, 1)` 随机向量，没有用真实 tokenizer/激活统计，因此读数
//! 只反映算子层面的量化误差，不代表 MiniLM 在真实语料上的端到端精度。

use serde_json::{Map, Value, json};
use trit_wspace::safetensors::{self, Weights};

const SUFFIXES: [(&str, &str); 3] = [
    ("query", "attention.self.query"),
    ("intermediate", "intermediate.dense"),
    ("output", "output.dense"),
];

/// 一个 f32 线性层：`y = W·x + b`。
struct F32Layer {
    name: String,
    rows: usize,
    cols: usize,
    weight: Vec<f32>,
    bias: Vec<f32>,
}

/// 三值化的对照层。
struct QuantLayer {
    rows: usize,
    cols: usize,
    trits: Vec<i8>,
    scales: Vec<f32>,
    bias_trit: Vec<i8>,
    bias_f32: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BiasMode {
    Zero,
    Trit,
    F32,
}

impl BiasMode {
    const ALL: [Self; 3] = [Self::Zero, Self::Trit, Self::F32];

    const fn key(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::Trit => "trit",
            Self::F32 => "f32",
        }
    }
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// `[-1, 1)` 的均匀分布（确定性，便于复算）。
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() % 2001) as f32 / 1000.0 - 1.0
    }
}

impl F32Layer {
    fn forward(&self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.rows);
        for row in 0..self.rows {
            let mut acc = f64::from(self.bias[row]);
            let weights = &self.weight[row * self.cols..(row + 1) * self.cols];
            for (&weight, &value) in weights.iter().zip(input) {
                acc += f64::from(weight) * f64::from(value);
            }
            out.push(acc as f32);
        }
    }

    fn quantize(&self) -> QuantLayer {
        let weights = Weights {
            rows: self.rows,
            cols: self.cols,
            values: self.weight.clone(),
        };
        let layer = weights
            .to_layer_with_bias(Some(&self.bias))
            .expect("三值化失败");
        let mut trits = Vec::with_capacity(self.rows * self.cols);
        let mut scales = Vec::with_capacity(self.rows);
        let mut bias_trit = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            scales.push(layer.scale(row).expect("scale"));
            bias_trit.push(layer.bias_trit(row).expect("bias").to_i8());
            for col in 0..self.cols {
                trits.push(layer.trit(row, col).expect("trit").to_i8());
            }
        }
        QuantLayer {
            rows: self.rows,
            cols: self.cols,
            trits,
            scales,
            bias_trit,
            bias_f32: self.bias.clone(),
        }
    }
}

impl QuantLayer {
    fn forward(&self, input: &[f32], mode: BiasMode, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.rows);
        for row in 0..self.rows {
            let mut acc = match mode {
                BiasMode::Zero => 0.0,
                BiasMode::Trit => f64::from(self.bias_trit[row]),
                BiasMode::F32 => 0.0,
            };
            let trits = &self.trits[row * self.cols..(row + 1) * self.cols];
            for (&trit, &value) in trits.iter().zip(input) {
                acc += f64::from(trit) * f64::from(value);
            }
            let mut value = f64::from(self.scales[row]) * acc;
            if mode == BiasMode::F32 {
                value += f64::from(self.bias_f32[row]);
            }
            out.push(value as f32);
        }
    }
}

/// 汇总误差：所有样本、所有输出元素上的累计量。
#[derive(Default)]
struct Acc {
    sq_err: f64,
    ref_sq: f64,
    out_sq: f64,
    dot: f64,
    count: u64,
}

impl Acc {
    fn add(&mut self, output: &[f32], reference: &[f32]) {
        for (&out, &reference) in output.iter().zip(reference) {
            let out = f64::from(out);
            let reference = f64::from(reference);
            let diff = out - reference;
            self.sq_err += diff * diff;
            self.ref_sq += reference * reference;
            self.out_sq += out * out;
            self.dot += out * reference;
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
        let cosine = if denominator > 0.0 {
            self.dot / denominator
        } else {
            1.0
        };
        json!({
            "mse": mse,
            "rmse": mse.sqrt(),
            "rel_l2": rel_l2,
            "cosine": cosine,
        })
    }
}

fn run_chain(
    f32_layers: &[F32Layer],
    quant_layers: &[QuantLayer],
    modes: &[Option<BiasMode>],
    input: &[f32],
) -> Vec<f32> {
    let mut current = input.to_vec();
    let mut next = Vec::new();
    for (index, f32_layer) in f32_layers.iter().enumerate() {
        match modes[index] {
            Some(mode) => quant_layers[index].forward(&current, mode, &mut next),
            None => f32_layer.forward(&current, &mut next),
        }
        std::mem::swap(&mut current, &mut next);
    }
    current
}

fn reference_outputs(f32_layers: &[F32Layer], inputs: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let modes = vec![None; f32_layers.len()];
    inputs
        .iter()
        .map(|input| run_chain(f32_layers, &[], &modes, input))
        .collect()
}

fn candidate_outputs(
    f32_layers: &[F32Layer],
    quant_layers: &[QuantLayer],
    modes: &[Option<BiasMode>],
    inputs: &[Vec<f32>],
) -> Vec<Vec<f32>> {
    inputs
        .iter()
        .map(|input| run_chain(f32_layers, quant_layers, modes, input))
        .collect()
}

fn compare(references: &[Vec<f32>], outputs: &[Vec<f32>]) -> Value {
    let mut acc = Acc::default();
    for (output, reference) in outputs.iter().zip(references) {
        acc.add(output, reference);
    }
    acc.finish()
}

fn random_inputs(cols: usize, count: usize, seed: u64) -> Vec<Vec<f32>> {
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|_| (0..cols).map(|_| rng.next_f32()).collect())
        .collect()
}

fn synthetic_layers(layers: usize) -> Vec<F32Layer> {
    let specs = [(64usize, 64usize), (128, 64), (64, 128)];
    let mut out = Vec::with_capacity(layers * specs.len());
    for layer in 0..layers {
        for (index, &(rows, cols)) in specs.iter().enumerate() {
            let seed =
                0x5eed_2026 ^ ((layer * specs.len() + index) as u64).wrapping_mul(0x9e37_79b9);
            let mut rng = Rng::new(seed);
            out.push(F32Layer {
                name: format!("layer{layer}.{}", SUFFIXES[index].0),
                rows,
                cols,
                weight: (0..rows * cols).map(|_| rng.next_f32() * 0.1).collect(),
                bias: (0..rows).map(|_| rng.next_f32() * 0.05).collect(),
            });
        }
    }
    out
}

fn load_layers(file: &str, layer_index: usize, layers: usize) -> Vec<F32Layer> {
    let mut out = Vec::with_capacity(layers * SUFFIXES.len());
    for offset in 0..layers {
        let index = layer_index + offset;
        for &(name, suffix) in &SUFFIXES {
            let weight_name = format!("encoder.layer.{index}.{suffix}.weight");
            let bias_name = format!("encoder.layer.{index}.{suffix}.bias");
            let weight = safetensors::read_tensor(file, &weight_name)
                .unwrap_or_else(|error| panic!("读取 {weight_name} 失败：{error}"));
            let bias = safetensors::read_vector(file, &bias_name)
                .unwrap_or_else(|error| panic!("读取 {bias_name} 失败：{error}"));
            assert_eq!(bias.len(), weight.rows);
            out.push(F32Layer {
                name: format!("layer{index}.{name}"),
                rows: weight.rows,
                cols: weight.cols,
                weight: weight.values,
                bias,
            });
        }
    }
    out
}

fn layer_info(layer: &F32Layer, quant: &QuantLayer) -> Value {
    let scale_mean =
        quant.scales.iter().map(|&s| f64::from(s)).sum::<f64>() / quant.scales.len().max(1) as f64;
    let bias_abs_mean = layer.bias.iter().map(|&b| f64::from(b.abs())).sum::<f64>()
        / layer.bias.len().max(1) as f64;
    json!({
        "name": &layer.name,
        "rows": layer.rows,
        "cols": layer.cols,
        "trits": quant.trits.len(),
        "nonzero_trits": quant.trits.iter().filter(|&&t| t != 0).count(),
        "scale_mean": scale_mean,
        "bias_abs_mean": bias_abs_mean,
        "bias_nonzero": layer.bias.iter().filter(|&&b| b != 0.0).count(),
    })
}

fn main() {
    let mut file = String::new();
    let mut layer_index = 0usize;
    let mut inputs = 256usize;
    let mut layer_count = 1usize;
    let mut swap = false;
    let mut seed = 7u64;
    let mut out_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--file" => file = args.next().expect("file"),
            "--layer" => layer_index = args.next().expect("layer").parse().expect("num"),
            "--inputs" => inputs = args.next().expect("inputs").parse().expect("num"),
            "--layers" => layer_count = args.next().expect("layers").parse().expect("num"),
            "--swap" => swap = true,
            "--seed" => seed = args.next().expect("seed").parse().expect("num"),
            "--out" => out_path = Some(args.next().expect("out")),
            other => panic!("unknown arg {other}"),
        }
    }
    assert!(inputs > 0, "--inputs 必须大于 0");
    assert!(layer_count > 0, "--layers 必须大于 0");

    let (f32_layers, source) = if file.is_empty() {
        (
            synthetic_layers(layer_count),
            json!({"kind": "synthetic", "layers": layer_count}),
        )
    } else {
        (
            load_layers(&file, layer_index, layer_count),
            json!({"kind": "safetensors", "path": file, "layer": layer_index, "layers": layer_count}),
        )
    };
    let quant_layers: Vec<QuantLayer> = f32_layers.iter().map(F32Layer::quantize).collect();

    // 逐层：每层独立，随机输入，参照该层自己的 f32 输出。
    let mut layers = Vec::with_capacity(f32_layers.len());
    for (index, layer) in f32_layers.iter().enumerate() {
        let samples = random_inputs(layer.cols, inputs, seed ^ (index as u64));
        let single_f32 = std::slice::from_ref(layer);
        let single_quant = std::slice::from_ref(&quant_layers[index]);
        let references = reference_outputs(single_f32, &samples);
        let mut readouts = Map::new();
        for mode in BiasMode::ALL {
            let modes = [Some(mode)];
            let outputs = candidate_outputs(single_f32, single_quant, &modes, &samples);
            readouts.insert(mode.key().to_string(), compare(&references, &outputs));
        }
        let mut info = layer_info(layer, &quant_layers[index]);
        info["readouts"] = Value::Object(readouts);
        layers.push(info);
    }

    // 端到端：三层全部三值化；再逐层替换，分离单层贡献。
    let end_to_end_inputs = random_inputs(f32_layers[0].cols, inputs, seed ^ 0x9e37_79b9_7f4a_7c15);
    let references = reference_outputs(&f32_layers, &end_to_end_inputs);
    let mut cascade = Map::new();
    for mode in BiasMode::ALL {
        let modes = vec![Some(mode); f32_layers.len()];
        let outputs = candidate_outputs(&f32_layers, &quant_layers, &modes, &end_to_end_inputs);
        cascade.insert(mode.key().to_string(), compare(&references, &outputs));
    }
    let mut swaps = Vec::new();
    if swap || layer_count == 1 {
        swaps.reserve(f32_layers.len());
        for index in 0..f32_layers.len() {
            let mut readouts = Map::new();
            for mode in BiasMode::ALL {
                let mut modes = vec![None; f32_layers.len()];
                modes[index] = Some(mode);
                let outputs =
                    candidate_outputs(&f32_layers, &quant_layers, &modes, &end_to_end_inputs);
                readouts.insert(mode.key().to_string(), compare(&references, &outputs));
            }
            swaps.push(json!({
                "layer": &f32_layers[index].name,
                "readouts": Value::Object(readouts),
            }));
        }
    }

    let report = json!({
        "source": source,
        "inputs": inputs,
        "seed": seed,
        "input_distribution": "uniform(-1, 1)",
        "bias_modes": ["zero", "trit", "f32"],
        "note": "输入是随机向量，不是真实 tokenizer/激活统计；读数只反映算子层面的量化误差。",
        "layers": layers,
        "cascade": cascade,
        "single_layer_swap": swaps,
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    match out_path {
        Some(path) => {
            std::fs::write(&path, text + "\n").expect("写报告失败");
            println!("写入 {path}");
        }
        None => println!("{text}"),
    }
}
