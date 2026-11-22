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

mod activation;
mod linear;
mod model;
mod report;
mod scales;
mod stats;
mod sts;

use serde_json::{Map, json};
use trit_wspace::activations;

use activation::ActivationMode;
use model::{Bert, BertConfig};
use report::{compare, layer_refs};
use scales::load_scales;
use stats::{input_stats, random_like};
use sts::{load_pairs, metrics, sentence_vectors};

fn main() {
    let mut file = String::new();
    let mut data_dir = String::new();
    let mut out_path = None;
    let mut seed = 7u64;
    let mut activation_arg = "all".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--file" => file = args.next().expect("file"),
            "--data-dir" => data_dir = args.next().expect("data-dir"),
            "--out" => out_path = Some(args.next().expect("out")),
            "--seed" => seed = args.next().expect("seed").parse().expect("num"),
            "--activation-mode" => activation_arg = args.next().expect("activation-mode"),
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

    let pairs = load_pairs(&data_dir);
    let scales = load_scales(&data_dir);
    let (heads, eps) = (12usize, 1e-12f32);
    let requested = (activation_arg != "all").then(|| ActivationMode::parse(&activation_arg));
    let f32_bert = Bert::load(
        &file,
        BertConfig {
            layers: 6,
            hidden,
            intermediate: hidden * 4,
            heads,
            eps,
            quantize: false,
            activation: ActivationMode::F32,
        },
    );
    let (f32_out, _, f32_layers) = f32_bert.forward(&input, &mask, batch, seq, false);
    let f32_sts = metrics(
        &sentence_vectors(&f32_out, &mask, batch, seq, hidden),
        &pairs,
    );
    let random_input = random_like(&input, seed);
    let (f32_random, _, _) = f32_bert.forward(&random_input, &mask, batch, seq, false);

    let mut modes = Map::new();
    let mut all_stages = None;
    for mode in [
        ActivationMode::F32,
        ActivationMode::TernaryLn,
        ActivationMode::TernaryAll,
        ActivationMode::TernaryAllScale,
        ActivationMode::TernaryAllChannel,
    ] {
        if requested.is_some_and(|wanted| wanted != mode) {
            continue;
        }
        let mut quant = Bert::load(
            &file,
            BertConfig {
                layers: 6,
                hidden,
                intermediate: hidden * 4,
                heads,
                eps,
                quantize: true,
                activation: mode,
            },
        );
        if mode == ActivationMode::TernaryAllChannel {
            quant.set_scales(scales.clone());
        }
        let (real_out, real_stages, _) = quant.forward(&input, &mask, batch, seq, true);
        let (random_out, random_stages, _) = quant.forward(&random_input, &mask, batch, seq, true);
        modes.insert(
            mode.key().to_string(),
            json!({
                "real_vs_f32": compare(&real_out, &f32_out, &mask, hidden),
                "real_vs_reference": compare(&real_out, &reference, &mask, hidden),
                "random_vs_f32": compare(&random_out, &f32_random, &mask, hidden),
                "sts": metrics(&sentence_vectors(&real_out, &mask, batch, seq, hidden), &pairs),
            }),
        );
        if mode == ActivationMode::TernaryAll {
            all_stages = Some((real_stages, random_stages));
        }
    }

    let layer_refs = layer_refs(&data_dir, &f32_layers, &mask, hidden);
    let report = json!({
        "source": {
            "seq": seq,
            "hidden": hidden,
            "heads": heads,
            "activation_mode": activation_arg,
            "pairs": pairs.len(),
        },
        "f32_vs_reference": compare(&f32_out, &reference, &mask, hidden),
        "f32_layers_vs_reference": layer_refs,
        "f32_sts": f32_sts,
        "modes": modes,
        "output_stage": match all_stages {
            Some((real, random)) => json!({
                "real": real.iter().map(|s| input_stats(s)).collect::<Vec<_>>(),
                "random": random.iter().map(|s| input_stats(s)).collect::<Vec<_>>(),
            }),
            None => json!(null),
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
