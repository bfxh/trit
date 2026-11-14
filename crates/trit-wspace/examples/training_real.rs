//! 真实数据子网络上的训练历史：MiniLM query 的 2×2 子矩阵 + 词向量前 2 维。
//!
//! 每轮 `repair` 后记录 loss 与输入侧逐位敏感度（有限差分「梯度」代理）。

use serde_json::json;
use trit_wspace::agree;
use trit_wspace::repair::RepairOptions;
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::training::run;
use trit_wspace::{Edit, Layer, Net, Oracle, Patch, Trit};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const SAMPLES: usize = 32;
const DIM: usize = 4;

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
        .expect("用法: training_real <model.safetensors>");
    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let sub: Vec<f32> = (0..DIM)
        .flat_map(|row| {
            let offset = row * query.cols;
            query.values[offset..offset + DIM].to_vec()
        })
        .collect();
    let sub_weights = Weights {
        rows: DIM,
        cols: DIM,
        values: sub.clone(),
    };
    let readout: Vec<f32> = sub[..DIM].to_vec();
    let mut net = Net::new(vec![
        sub_weights.to_layer().expect("layer"),
        Layer::new(1, DIM, &trit_row_i8(&readout), &[0]).expect("readout"),
    ])
    .expect("net");
    let samples = SAMPLES.min(embeddings.rows);
    let inputs: Vec<Vec<Trit>> = (0..samples)
        .map(|row| {
            let offset = row * embeddings.cols;
            trit_row(&embeddings.values[offset..offset + DIM])
        })
        .collect();
    let outputs: Vec<Vec<Trit>> = (0..samples)
        .map(|row| {
            let offset = row * embeddings.cols;
            vec![agree::reference(
                &sub_weights,
                &readout,
                &embeddings.values[offset..offset + DIM],
            )]
        })
        .collect();
    let oracle = Oracle::new(inputs, outputs).expect("oracle");
    Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Zero,
    }])
    .apply(&mut net)
    .expect("break");
    let options = RepairOptions {
        max_edits: 2,
        beam_width: 8,
        ..RepairOptions::default()
    };
    let history = run(&mut net, &oracle, 3, &options).expect("run");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "samples": samples,
            "dim": DIM,
            "rounds": history.rounds.iter().map(|round| json!({
                "round": round.round,
                "before_loss": round.before_loss,
                "after_loss": round.after_loss,
                "patch_len": round.patch_len,
                "flip_rate": round.flip_rate,
                "lost_rate": round.lost_rate,
                "max_position_flips": round.max_position_flips,
            })).collect::<Vec<_>>(),
        }))
        .expect("json")
    );
}
