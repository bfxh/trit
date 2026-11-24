//! 训练步基准：标量参考 vs 位平面前向/`dW` 融合更新。

use std::hint::black_box;
use std::time::Instant;

use trit_train::layer::TernaryLinear;
use trit_train::quant::Ste;

/// 单样本训练一步：标量参考 vs 位平面前向/scale 梯度，返回
/// `(标量 ms/step, 位平面 ms/step, 加速比, pack ms/step)`。
///
/// 计时含 `pack()`、前向、`dW` 更新与 scale 更新；不含层构造和输入生成。
pub(super) fn bench_train_step(
    rows: usize,
    cols: usize,
    steps: usize,
    rounds: usize,
) -> (f64, f64, f64, f64) {
    let build = || {
        let mut state = 0xdead_beef_0bad_f00du64;
        let mut layer = TernaryLinear::new(
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
        );
        layer.set_prefetch(false);
        layer
    };
    let mut data = 0x0123_4567_89ab_cdefu64;
    let x: Vec<f32> = (0..cols)
        .map(|_| {
            data ^= data << 13;
            data ^= data >> 7;
            data ^= data << 17;
            ((data % 2001) as f32 - 1000.0) / 1000.0
        })
        .collect();
    let target: Vec<f32> = (0..rows).map(|r| ((r % 13) as f32 - 6.0) * 0.2).collect();
    let mut best_scalar = f64::INFINITY;
    let mut best_plane = f64::INFINITY;
    let mut best_pack = f64::INFINITY;
    for _ in 0..rounds.max(1) {
        let mut scalar = build();
        let mut plane = build();
        let start = Instant::now();
        for _ in 0..steps.max(1) {
            black_box(scalar.train_step_scalar(&x, &target));
        }
        best_scalar = best_scalar.min(start.elapsed().as_secs_f64());
        let start = Instant::now();
        for _ in 0..steps.max(1) {
            black_box(plane.train_step(&x, &target));
        }
        best_plane = best_plane.min(start.elapsed().as_secs_f64());
        let start = Instant::now();
        for _ in 0..steps.max(1) {
            let planes = plane.pack();
            black_box((planes.q(), planes.bytes()));
        }
        best_pack = best_pack.min(start.elapsed().as_secs_f64());
        black_box((scalar.latent(), plane.latent()));
    }
    let steps = steps.max(1) as f64;
    let scalar_ms = best_scalar * 1000.0 / steps;
    let plane_ms = best_plane * 1000.0 / steps;
    (
        scalar_ms,
        plane_ms,
        scalar_ms / plane_ms,
        best_pack * 1000.0 / steps,
    )
}
