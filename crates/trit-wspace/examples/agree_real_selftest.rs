//! 舍入边界自检：库的 absmean 三值化（f32）与 f64 诊断口径是否逐位一致。
//!
//! 覆盖 `-0/+0`、半整数、抵消行、全零行、单点行与随机行；另外验证
//! `Layer::forward` 在零累加时输出 `Zero`。不依赖模型文件：
//! `cargo run --release -p trit-wspace --example agree_real_selftest`

use serde_json::json;
use trit_wspace::safetensors::Weights;
use trit_wspace::{Layer, Net, Trit};

/// f64 参考口径：absmean 三值化（丢 scale）。
fn q64(row: &[f64]) -> Vec<i8> {
    let scale = row.iter().map(|v| v.abs()).sum::<f64>() / row.len().max(1) as f64;
    row.iter()
        .map(|v| (v / scale.max(f64::MIN_POSITIVE)).round().clamp(-1.0, 1.0) as i8)
        .collect()
}

/// 库口径：`Weights::to_layer()` 的 f32 absmean。
fn lib_q(row: &[f32]) -> Vec<i8> {
    let w = Weights {
        rows: 1,
        cols: row.len(),
        values: row.to_vec(),
    };
    let layer = w.to_layer().expect("ternarize");
    (0..row.len())
        .map(|c| layer.trit(0, c).expect("trit").to_i8())
        .collect()
}

/// LCG，避免引入额外依赖。
fn next(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*seed >> 11) as f64) / ((1u64 << 53) as f64)
}

fn main() {
    let strict = std::env::args().any(|a| a == "--strict");
    let mut seed = 0x5eed_u64;
    let mut cases = 0usize;
    let mut bad = 0usize;
    let mut samples = Vec::new();

    // 边界行：±0、半整数、抵消、全零、单点、非精确小数
    let mut rows: Vec<Vec<f32>> = vec![
        vec![0.0, -0.0],
        vec![0.5, 0.5],
        vec![1.5, -1.5],
        vec![1.0, -1.0],
        vec![1e-30, -1e-30],
        vec![0.0; 5],
        vec![1.0],
        vec![0.1, 0.2, 0.3],
        vec![0.5, 1.5],
        vec![-0.5, 0.5, 1.5],
    ];
    // 随机行：长度 1..=33，值域 [-2, 2)
    for len in 1..=33usize {
        for _ in 0..64 {
            let row: Vec<f32> = (0..len)
                .map(|_| ((next(&mut seed) * 4.0) - 2.0) as f32)
                .collect();
            rows.push(row);
        }
    }

    for row in &rows {
        cases += 1;
        let want = q64(&row.iter().map(|v| f64::from(*v)).collect::<Vec<_>>());
        let got = lib_q(row);
        if want != got {
            bad += 1;
            if samples.len() < 8 {
                samples.push(json!({ "row": row, "want": want, "got": got }));
            }
        }
    }

    // 零累加：库里 Σ=0 必须输出 Zero，不能随机 tie-break
    let w = [1i8, -1, 1, -1];
    let x = [1i8, 1, -1, 1];
    let layer = Layer::new(1, 4, &w, &[0]).expect("layer");
    let net = Net::new(vec![layer]).expect("net");
    let input: Vec<Trit> = x.iter().map(|v| Trit::from_i8(*v).expect("trit")).collect();
    let got = net.forward(&input).expect("forward");
    let acc: i32 = w
        .iter()
        .zip(&x)
        .map(|(a, b)| i32::from(*a) * i32::from(*b))
        .sum();
    let want = if acc > 0 {
        Trit::Pos
    } else if acc < 0 {
        Trit::Neg
    } else {
        Trit::Zero
    };
    cases += 1;
    if got != vec![want] {
        bad += 1;
        let got_i8: Vec<i8> = got.iter().map(|t| t.to_i8()).collect();
        samples.push(json!({ "zero_acc": acc, "got": got_i8 }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "cases": cases,
            "mismatches": bad,
            "samples": samples,
        }))
        .expect("json")
    );
    if bad > 0 && strict {
        std::process::exit(1);
    }
}
