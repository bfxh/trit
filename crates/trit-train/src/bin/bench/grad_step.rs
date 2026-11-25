//! 带梯度 SGD 训练步基准：`train_step_with_grad` 的整步耗时。

use std::hint::black_box;
use std::time::Instant;

use trit_train::layer::TernaryLinear;
use trit_train::quant::Ste;

/// 测 `train_step_with_grad`：输出梯度已给定，不含层构造/输入生成。
pub(super) fn bench_grad_step(rows: usize, cols: usize, steps: usize, rounds: usize) {
    let build = || {
        let mut state = 0x1357_9bdf_2468_ace0u64;
        TernaryLinear::new(
            rows,
            cols,
            |_, _| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state % 100) as f32 - 50.0) * 0.001
            },
            Ste::Soft,
            0.01,
            0.5,
        )
    };
    let mut data = 0x0f1e_2d3c_4b5a_6978u64;
    let x: Vec<f32> = (0..cols)
        .map(|_| {
            data ^= data << 13;
            data ^= data >> 7;
            data ^= data << 17;
            ((data % 2001) as f32 - 1000.0) / 1000.0
        })
        .collect();
    let dy: Vec<f32> = (0..rows).map(|r| ((r % 17) as f32 - 8.0) * 0.01).collect();
    let mut best = f64::INFINITY;
    for _ in 0..rounds.max(1) {
        let mut layer = build();
        let start = Instant::now();
        for _ in 0..steps.max(1) {
            black_box(layer.train_step_with_grad(&x, &dy));
        }
        best = best.min(start.elapsed().as_secs_f64());
        black_box(layer.latent());
    }
    let ms = best * 1000.0 / steps.max(1) as f64;
    println!("\n带梯度 SGD：{rows}×{cols}（{steps} 步/轮，{rounds} 轮取最快）");
    println!("train_step_with_grad  {ms:.2} ms/step");
}
