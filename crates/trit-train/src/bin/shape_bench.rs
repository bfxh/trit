//! 训练步的多形状矩阵：SGD `train_step` 与 Adam `train_step_with_grad_adam`。
//!
//! 每个形状分别跑融合路径与逐元素/标量参考，输出 `ms/step` 和相对倍数；
//! 计时口径与 `train-bench` 一致：`pack()` + 前向/dX + `dW` + scale，构造和输入生成不计。

use std::hint::black_box;
use std::time::Instant;

use trit_train::layer::{Adam, TernaryLinear};
use trit_train::quant::Ste;

type SgdStep = fn(&mut TernaryLinear, &[f32], &[f32]) -> f32;
type AdamStep = fn(&mut TernaryLinear, &[f32], &[f32], &mut Adam, &mut Adam) -> Vec<f32>;

const SHAPES: [(usize, usize); 7] = [
    (1, 1),
    (2, 7),
    (5, 9),
    (9, 65),
    (11, 130),
    (17, 576),
    (4096, 576),
];

fn build(rows: usize, cols: usize) -> TernaryLinear {
    TernaryLinear::new(
        rows,
        cols,
        |r, c| {
            let v = (r * 37 + c * 11 + 5) % 101;
            (v as f32 - 50.0) / 50.0
        },
        Ste::Soft,
        0.01,
        0.5,
    )
}

fn inputs(rows: usize, cols: usize) -> (Vec<f32>, Vec<f32>) {
    let x = (0..cols)
        .map(|c| ((c * 29 + 7) % 67) as f32 / 33.0 - 1.0)
        .collect();
    let dy = (0..rows)
        .map(|r| ((r * 17 + 3) % 41) as f32 / 20.0 - 1.0)
        .collect();
    (x, dy)
}

fn targets(rows: usize) -> Vec<f32> {
    (0..rows)
        .map(|r| ((r * 13 + 9) % 73) as f32 / 36.0 - 1.0)
        .collect()
}

fn run_sgd(rows: usize, cols: usize, steps: usize, rounds: usize, scalar: bool) -> f64 {
    let (x, _) = inputs(rows, cols);
    let target = targets(rows);
    let step: SgdStep = if scalar {
        TernaryLinear::train_step_scalar
    } else {
        TernaryLinear::train_step
    };
    let mut best = f64::INFINITY;
    for _ in 0..rounds.max(1) {
        let mut layer = build(rows, cols);
        let start = Instant::now();
        for _ in 0..steps.max(1) {
            black_box(step(&mut layer, &x, &target));
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    best * 1000.0 / steps.max(1) as f64
}

fn run(rows: usize, cols: usize, steps: usize, rounds: usize, scalar: bool) -> f64 {
    let (x, dy) = inputs(rows, cols);
    let step: AdamStep = if scalar {
        TernaryLinear::train_step_with_grad_adam_scalar
    } else {
        TernaryLinear::train_step_with_grad_adam
    };
    let mut best = f64::INFINITY;
    for _ in 0..rounds.max(1) {
        let mut layer = build(rows, cols);
        let mut latent_opt = Adam::new(rows * cols, 0.01);
        let mut scale_opt = Adam::new(rows, 0.01);
        let start = Instant::now();
        for _ in 0..steps.max(1) {
            black_box(step(&mut layer, &x, &dy, &mut latent_opt, &mut scale_opt));
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    best * 1000.0 / steps.max(1) as f64
}

fn steps_for(rows: usize, cols: usize) -> usize {
    if rows * cols <= 100 {
        100_000
    } else if rows * cols <= 10_000 {
        5_000
    } else {
        3
    }
}

fn main() {
    let rounds = 9;
    println!("SGD 多形状矩阵（按形状放大步数，{rounds} 轮取最快，含 pack）");
    println!(
        "{:<12} {:>12} {:>12} {:>10} {:>8}",
        "shape", "位平面 ms", "标量 ms", "倍数", "步/轮"
    );
    for (rows, cols) in SHAPES {
        let steps = steps_for(rows, cols);
        let fused = run_sgd(rows, cols, steps, rounds, false);
        let scalar = run_sgd(rows, cols, steps, rounds, true);
        let ratio = scalar / fused;
        println!(
            "{:<12} {fused:>12.4} {scalar:>12.4} {ratio:>9.2}× {steps:>8}",
            format!("{rows}×{cols}")
        );
    }

    println!("\nAdam 多形状矩阵（按形状放大步数，{rounds} 轮取最快，含 pack）");
    println!(
        "{:<12} {:>12} {:>12} {:>10} {:>8}",
        "shape", "融合 ms", "逐元素 ms", "倍数", "步/轮"
    );
    for (rows, cols) in SHAPES {
        let steps = steps_for(rows, cols);
        let fused = run(rows, cols, steps, rounds, false);
        let scalar = run(rows, cols, steps, rounds, true);
        let ratio = scalar / fused;
        println!(
            "{:<12} {fused:>12.4} {scalar:>12.4} {ratio:>9.2}× {steps:>8}",
            format!("{rows}×{cols}")
        );
    }
}
