//! 真实规模统计：128 个词向量行，cost / perturb / accuracy 的 bootstrap 95% CI。
//!
//! 指标都按样本计算，再用有放回重采样（LCG，无外部依赖）给出置信区间。

use serde_json::json;
use trit_wspace::agree;
use trit_wspace::cost::cost;
use trit_wspace::perturb::{PerturbMode, perturb};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Net, Oracle, Trit};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const SAMPLES: usize = 2048;
const BOOTSTRAP: usize = 1000;

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

/// LCG：`[0, 1)` 均匀。
fn next(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*seed >> 11) as f64) / ((1u64 << 53) as f64)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn bootstrap(values: &[f64], rounds: usize, seed: u64) -> (f64, f64, f64) {
    let mean = if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    };
    if values.is_empty() {
        return (mean, mean, mean);
    }
    let mut rng = seed;
    let mut means = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let mut sum = 0.0;
        for _ in 0..values.len() {
            let index = (next(&mut rng) * values.len() as f64) as usize;
            sum += values[index.min(values.len() - 1)];
        }
        means.push(sum / values.len() as f64);
    }
    means.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    (mean, percentile(&means, 0.025), percentile(&means, 0.975))
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: real_stats <model.safetensors>");
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
    let actual = oracle.outputs_of(&net).expect("outputs");
    let want = oracle.outputs();
    let correct: Vec<f64> = (0..oracle.len())
        .map(|index| f64::from(actual[index] == want[index]))
        .collect();
    let cost_report = cost(&net, &oracle, oracle.len()).expect("cost");
    let perturb_report = perturb(&net, &oracle, PerturbMode::Flip).expect("perturb");
    let width = perturb_report.width as f64;
    let multiplies: Vec<f64> = cost_report
        .top
        .iter()
        .map(|sample| sample.multiplies as f64)
        .collect();
    let flip_rate: Vec<f64> = perturb_report
        .per_sample
        .iter()
        .map(|sample| sample.flips as f64 / width)
        .collect();
    let lost_rate: Vec<f64> = perturb_report
        .per_sample
        .iter()
        .map(|sample| sample.lost as f64 / width)
        .collect();
    let (accuracy, acc_lo, acc_hi) = bootstrap(&correct, BOOTSTRAP, 0x5eed);
    let (cost_mean, cost_lo, cost_hi) = bootstrap(&multiplies, BOOTSTRAP, 0xbeef);
    let (flip_mean, flip_lo, flip_hi) = bootstrap(&flip_rate, BOOTSTRAP, 0xcafe);
    let (lost_mean, lost_lo, lost_hi) = bootstrap(&lost_rate, BOOTSTRAP, 0xd00d);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "samples": samples,
            "bootstrap": BOOTSTRAP,
            "accuracy": {"mean": accuracy, "ci": [acc_lo, acc_hi]},
            "cost_mean": {"mean": cost_mean, "ci": [cost_lo, cost_hi]},
            "flip_rate": {"mean": flip_mean, "ci": [flip_lo, flip_hi]},
            "lost_rate": {"mean": lost_mean, "ci": [lost_lo, lost_hi]},
        }))
        .expect("json")
    );
}
