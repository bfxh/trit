//! 真实词向量输入上的压缩保真度：`project --agree-from` 的实验版（不经过 CLI）。
//!
//! 只测两个端点——`k=1`（判据方向 `w_eff`）与 `k=384`（恒等），回答「压到一维在真实输入上
//! 究竟保住多少一致率」。输入是 `word_embeddings` 的真实行（三值化），不是随机三值。

use serde_json::json;
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Net, Trit};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";

fn trit_row(values: &[f32]) -> Vec<Trit> {
    let weights = Weights {
        rows: 1,
        cols: values.len(),
        values: values.to_vec(),
    };
    let layer = weights.to_layer().expect("ternarize");
    (0..values.len()).map(|c| layer.trit(0, c).expect("trit")).collect()
}

fn trit_row_i8(values: &[f32]) -> Vec<i8> {
    trit_row(values).iter().map(|t| t.to_i8()).collect()
}

fn main() {
    let path = std::env::args().nth(1).expect("用法: agree_real <model.safetensors>");
    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = query.cols;

    // 学生：query 三值层 + readout（query 第 0 行）
    let readout: Vec<f32> = query.values[..dim].to_vec();
    let student = Net::new(vec![
        query.to_layer().expect("ternary query"),
        Layer::new(1, dim, &trit_row_i8(&readout), &[0]).expect("readout"),
    ])
    .expect("student");

    // 判据方向 w_eff = queryᵀ readout（f32），三值化成 1×dim 的基
    let mut w_eff = vec![0.0f32; dim];
    for (row, r) in readout.iter().enumerate() {
        let line = &query.values[row * dim..(row + 1) * dim];
        for (slot, value) in w_eff.iter_mut().zip(line) {
            *slot += r * value;
        }
    }
    let basis_one =
        Net::new(vec![Layer::new(1, dim, &trit_row_i8(&w_eff), &[0]).expect("basis")]).expect("b1");

    // 真实输入：词表前 N 行（三值化）
    let samples = 1000usize.min(embeddings.rows);
    let inputs: Vec<Vec<Trit>> = (0..samples)
        .map(|row| trit_row(&embeddings.values[row * dim..(row + 1) * dim]))
        .collect();

    let mut curves = Vec::new();
    for (k, basis) in [(1usize, Some(basis_one.layer(0).expect("l").clone())), (dim, None)] {
        let projected = match &basis {
            Some(base) => {
                let first = student.layer(0).expect("l0").project(base).expect("project");
                Net::new(vec![first, student.layer(1).expect("l1").clone()]).expect("projected")
            }
            None => student.clone(),
        };
        let mut same = 0usize;
        for input in &inputs {
            let reduced: Vec<Trit> = match &basis {
                Some(base) => (0..k)
                    .map(|row| {
                        let mut acc: i32 = 0;
                        for (col, value) in input.iter().enumerate() {
                            let w = base.trit(row, col).map(|t| t.to_i8()).unwrap_or(0);
                            acc += i32::from(w) * i32::from(value.to_i8());
                        }
                        match acc.signum() {
                            1 => Trit::Pos,
                            -1 => Trit::Neg,
                            _ => Trit::Zero,
                        }
                    })
                    .collect(),
                None => input.clone(),
            };
            if student.forward(input).expect("f") == projected.forward(&reduced).expect("f") {
                same += 1;
            }
        }
        curves.push(json!({
            "k": k,
            "rate": same as f64 / inputs.len() as f64,
            "samples": inputs.len(),
        }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "input": "word_embeddings rows (real, ternarized)",
            "readout_row": 0,
            "curves": curves,
        }))
        .expect("json")
    );
}
