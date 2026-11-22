//! 真实模型上的端到端 bias 读数：f32 权重作参照，比较三值权重 + {零 / 三值 / f32} bias。
//!
//! 输入默认是 `uniform(-1, 1)` 随机向量；`--activations-dir` 可换成真实 MiniLM
//! 第 0 层三段线性输入的激活（`WSPACT1` 格式，见 `export_bias_activations.py`）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example bias_e2e -- \
//!   --file model.safetensors --layer 0 --inputs 256
//! cargo run --release -p trit-wspace --example bias_e2e -- \
//!   --file model.safetensors --layer 0 --inputs 256 \
//!   --activations-dir "$TRIT_DATA/miniLM-e2e-activations"
//! ```
//!
//! 边界：真实激活只替换每层的输入；级联段仍是三线性段（没有 attention/LN/GELU/残差）。

mod model;

use serde_json::{Map, Value, json};
use trit_wspace::activations;

use model::{
    BiasMode, F32Layer, QuantLayer, SUFFIXES, candidate_outputs, compare, layer_info, load_layers,
    random_inputs, reference_outputs, synthetic_layers,
};

fn main() {
    let mut file = String::new();
    let mut layer_index = 0usize;
    let mut inputs = 256usize;
    let mut layer_count = 1usize;
    let mut swap = false;
    let mut seed = 7u64;
    let mut out_path = None;
    let mut activations_dir: Option<String> = None;
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
            "--activations-dir" => activations_dir = Some(args.next().expect("dir")),
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

    if activations_dir.is_some() && (layer_index != 0 || layer_count != 1) {
        panic!("--activations-dir 只支持 --layer 0 --layers 1（导出的是第 0 层三段）");
    }
    let real_inputs: Option<Vec<Vec<Vec<f32>>>> = activations_dir.as_ref().map(|dir| {
        let mut all = Vec::with_capacity(SUFFIXES.len());
        for (index, &(name, _)) in SUFFIXES.iter().enumerate() {
            let path = format!("{dir}/{name}.f32");
            let (rows, cols, values) = activations::read_matrix(&path)
                .unwrap_or_else(|error| panic!("读 {path} 失败：{error}"));
            assert_eq!(cols, f32_layers[index].cols, "{name} 列数与权重不符");
            assert_eq!(rows, inputs, "{name} 样本数与 --inputs 不符");
            all.push(
                values
                    .chunks_exact(cols)
                    .map(|chunk| chunk.to_vec())
                    .collect(),
            );
        }
        all
    });

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
        info["readouts"] = Value::Object(readouts.clone());
        if let Some(real) = &real_inputs {
            let samples = &real[index];
            let references = reference_outputs(single_f32, samples);
            let mut real_readouts = Map::new();
            for mode in BiasMode::ALL {
                let modes = [Some(mode)];
                let outputs = candidate_outputs(single_f32, single_quant, &modes, samples);
                real_readouts.insert(mode.key().to_string(), compare(&references, &outputs));
            }
            let mut ratios = Map::new();
            for mode in BiasMode::ALL {
                let key = mode.key();
                let random = &readouts[key];
                let real = &real_readouts[key];
                let random_l2 = random["rel_l2"].as_f64().expect("rel_l2");
                let real_l2 = real["rel_l2"].as_f64().expect("rel_l2");
                let random_cos = random["cosine"].as_f64().expect("cosine");
                let real_cos = real["cosine"].as_f64().expect("cosine");
                ratios.insert(
                    key.to_string(),
                    json!({
                        "rel_l2_ratio": if random_l2 > 0.0 { real_l2 / random_l2 } else { 0.0 },
                        "cosine_ratio": if random_cos > 0.0 { real_cos / random_cos } else { 0.0 },
                    }),
                );
            }
            info["readouts_real"] = Value::Object(real_readouts);
            info["real_vs_random"] = Value::Object(ratios);
        }
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
        "activation_source": match &activations_dir {
            Some(dir) => json!({"kind": "real", "dir": dir, "format": "WSPACT1", "samples": inputs}),
            None => json!({"kind": "uniform", "range": [-1.0, 1.0], "seed": seed}),
        },
        "input_distribution": if activations_dir.is_some() { "real MiniLM layer0 activations" } else { "uniform(-1, 1)" },
        "bias_modes": ["zero", "trit", "f32"],
        "note": if activations_dir.is_some() {
            "真实激活只替换每层输入；级联段仍是三线性段（没有 attention/LN/GELU/残差）。"
        } else {
            "输入是随机向量，不是真实 tokenizer/激活统计；读数只反映算子层面的量化误差。"
        },
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
