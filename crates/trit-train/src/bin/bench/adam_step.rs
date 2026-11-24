//! Adam 训练步基准：行级融合 `dW` 更新的单步耗时。

use std::hint::black_box;
use std::time::Instant;

use trit_train::layer::{Adam, TernaryLinear};

#[path = "adam_setup.rs"]
mod setup;

type AdamStep = fn(&mut TernaryLinear, &[f32], &[f32], &mut Adam, &mut Adam) -> Vec<f32>;

/// 单样本 Adam 训练一步并打印 `ms/step`；计时含量化、`dX`、`dW` 与 scale，不含 `pack()`。
/// `scalar` 为真时走逐元素参考路径，用于复算融合倍数的基线。
pub(super) fn bench_adam_step(rows: usize, cols: usize, steps: usize, rounds: usize, scalar: bool) {
    let (x, target) = setup::inputs(rows, cols);
    let step: AdamStep = if scalar {
        TernaryLinear::train_step_with_grad_adam_scalar
    } else {
        TernaryLinear::train_step_with_grad_adam
    };
    let mut best = f64::INFINITY;
    for _ in 0..rounds.max(1) {
        let mut layer = setup::build(rows, cols);
        let mut latent_opt = Adam::new(rows * cols, 0.01);
        let mut scale_opt = Adam::new(rows, 0.01);
        let start = Instant::now();
        for _ in 0..steps.max(1) {
            black_box(step(
                &mut layer,
                &x,
                &target,
                &mut latent_opt,
                &mut scale_opt,
            ));
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    let ms = best * 1000.0 / steps.max(1) as f64;
    let name = if scalar {
        "逐元素参考"
    } else {
        "融合 dW"
    };
    println!("\nAdam 训练步：{rows}×{cols}（{steps} 步/轮，{rounds} 轮取最快；量化单遍）");
    println!("{:<14} {:>10}", name, format!("{ms:.2} ms/step"));
}
