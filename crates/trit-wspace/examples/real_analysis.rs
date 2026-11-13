//! 真实词向量上的分析表：cost / perturb / trace / scan 的规模读数。
//!
//! 用 MiniLM 的 query 层 + readout 层构造 `Net`，输入是词表前 N 行（三值化），
//! 教师输出是 f32 判据 `sign(readout · (query·x))`。只跑 32 个样本，用于展示方法。

use serde_json::json;
use trit_wspace::agree;
use trit_wspace::cost::cost;
use trit_wspace::perturb::{PerturbMode, perturb};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::scan::{ScanOptions, scan};
use trit_wspace::trace::trace;
use trit_wspace::{Layer, Net, Oracle, Trit};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const SAMPLES: usize = 32;

fn trit_row(values: &[f32]) -> Vec<Trit> {
    let weights = Weights {
        rows: 1,
        cols: values.len(),
        values: values.to_vec(),
    };
    let layer = weights.to_layer().expect("ternarize");
    (0..values.len())
        .map(|col| layer.trit(0, col).expect("trit"))
        .collect()
}

fn trit_row_i8(values: &[f32]) -> Vec<i8> {
    trit_row(values).iter().map(|trit| trit.to_i8()).collect()
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: real_analysis <model.safetensors>");
    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = query.cols;
    let readout: Vec<f32> = query.values[..dim].to_vec();
    let net = Net::new(vec![
        query.to_layer().expect("query"),
        Layer::new(1, dim, &trit_row_i8(&readout), &[0]).expect("readout"),
    ])
    .expect("net");
    let samples = SAMPLES.min(embeddings.rows);
    let inputs: Vec<Vec<Trit>> = (0..samples)
        .map(|row| trit_row(&embeddings.values[row * dim..(row + 1) * dim]))
        .collect();
    let outputs: Vec<Vec<Trit>> = (0..samples)
        .map(|row| {
            vec![agree::reference(
                &query,
                &readout,
                &embeddings.values[row * dim..(row + 1) * dim],
            )]
        })
        .collect();
    let oracle = Oracle::new(inputs, outputs).expect("oracle");
    let cost_report = cost(&net, &oracle, 5).expect("cost");
    let perturb_report = perturb(&net, &oracle, PerturbMode::Flip).expect("perturb");
    let first = trace(&net, &oracle.inputs()[0]).expect("trace");
    let scan_report = scan(
        &net,
        &oracle,
        &ScanOptions {
            sample_budget: 16,
            max_params: 512,
            refine: 64,
            layer: Some(1),
            top: 8,
            ..ScanOptions::default()
        },
    )
    .expect("scan");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "samples": samples,
            "cost": {
                "total": cost_report.total,
                "mean": cost_report.mean,
                "max": cost_report.max,
                "min": cost_report.min,
                "per_layer": cost_report.per_layer,
                "top": cost_report.top.iter().map(|sample| json!({
                    "sample": sample.sample,
                    "multiplies": sample.multiplies,
                })).collect::<Vec<_>>(),
            },
            "perturb": {
                "perturbations": perturb_report.perturbations,
                "flipped": perturb_report.flipped,
                "lost": perturb_report.lost,
                "gained": perturb_report.gained,
                "top_positions": perturb_report.per_position.iter().take(5).map(|position| json!({
                    "position": position.position,
                    "flips": position.flips,
                })).collect::<Vec<_>>(),
            },
            "trace": {
                "layers": first.layers.iter().map(|layer| json!({
                    "layer": layer.layer,
                    "nonzero": layer.nonzero,
                })).collect::<Vec<_>>(),
                "output": first.output.iter().map(|trit| trit.to_i8()).collect::<Vec<_>>(),
            },
            "scan": {
                "stats": {
                    "total_params": scan_report.stats.total_params,
                    "screened": scan_report.stats.screened,
                    "refined": scan_report.stats.refined,
                    "sampled": scan_report.stats.sampled,
                },
                "capabilities": scan_report.report.capabilities.len(),
                "specialists": scan_report.report.specialists.len(),
            },
        }))
        .expect("json")
    );
}
