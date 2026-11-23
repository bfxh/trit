use crate::activation::ActivationMode;
use crate::model::{Bert, BertConfig};
use crate::report::{compare, layer_refs};
use crate::scales::load_scales;
use crate::stats::{input_stats, random_like};
use crate::sts::{load_pairs, metrics, sentence_vectors};
use args::Args;
use serde_json::json;
#[path = "args.rs"]
mod args;
#[path = "data.rs"]
mod data;
#[path = "modes.rs"]
mod modes;
#[path = "sidecar_svd.rs"]
mod sidecar_svd;
mod weight_format;

pub(crate) fn run() {
    let Args {
        file,
        data_dir,
        out: out_path,
        seed,
        activation: activation_arg,
        gptq,
        sidecar_svd: sidecar_ranks,
    } = args::parse();
    let (gptq_dir, gptq_group) = (gptq.map(|_| format!("{data_dir}/gptq")), gptq.unwrap_or(0));
    let data::Corpus {
        batch,
        seq,
        hidden,
        mask,
        input,
        reference,
    } = data::load(&data_dir);
    let pairs = load_pairs(&data_dir);
    let scales = load_scales(&data_dir);
    let (heads, eps) = (12usize, 1e-12f32);
    let requested = (activation_arg != "all").then(|| ActivationMode::parse(&activation_arg));
    let f32_bert = Bert::load(
        &file,
        BertConfig::new(hidden, eps, false, ActivationMode::F32),
    );
    let (f32_out, _, f32_layers) = f32_bert.forward(&input, &mask, batch, seq, data::keep_layers());
    let f32_sts = metrics(
        &sentence_vectors(&f32_out, &mask, batch, seq, hidden),
        &pairs,
    );
    let random_input = random_like(&input, seed);
    let (f32_random, _, _) = f32_bert.forward(&random_input, &mask, batch, seq, false);
    let (modes, all_stages) = modes::evaluate(&modes::Request {
        file: &file,
        gptq_dir: gptq_dir.clone(),
        gptq_group,
        hidden,
        eps,
        requested,
        input: &input,
        random_input: &random_input,
        mask: &mask,
        batch,
        seq,
        pairs: &pairs,
        f32_out: &f32_out,
        reference: &reference,
        f32_random: &f32_random,
        scales: &scales,
    });
    let layer_refs = layer_refs(&data_dir, &f32_layers, &mask, hidden);
    let sidecar = if activation_arg == "all" {
        crate::sidecar::run(
            &file,
            &input,
            &mask,
            &f32_out,
            &scales,
            &pairs,
            batch,
            seq,
            hidden,
            eps,
            &[0, 4, 8, 16],
        )
    } else {
        json!(null)
    };
    let sidecar_svd = sidecar_svd::run(
        sidecar_ranks,
        &sidecar_svd::Request {
            file: &file,
            gptq_dir: gptq_dir.clone(),
            gptq_group,
            mode: requested.unwrap_or(ActivationMode::F32),
            hidden,
            eps,
            input: &input,
            mask: &mask,
            batch,
            seq,
            pairs: &pairs,
            f32_out: &f32_out,
            scales: &scales,
        },
    );
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
        "weight_format": weight_format::report(hidden),
        "sidecar": sidecar,
        "sidecar_svd": sidecar_svd,
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
