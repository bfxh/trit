//! 复算 `TernaryLinear::forward` 与旧 `pack()+row_dot` 路径的差距。
//!
//! `cargo run -p trit-train --release --example forward_probe`

use std::hint::black_box;
use std::time::Instant;

use trit_train::Ste;
use trit_train::layer::TernaryLinear;

fn main() {
    let rows = 4096;
    let cols = 576;
    let layer = TernaryLinear::new(
        rows,
        cols,
        |r, c| ((r * 31 + c * 7) as f32 * 0.017).sin() * 0.9,
        Ste::Identity,
        0.0,
        0.5,
    );
    let x: Vec<f32> = (0..cols).map(|c| ((c * 13) as f32 * 0.011).cos()).collect();

    let mut best_old = f64::INFINITY;
    let mut best_new = f64::INFINITY;
    for _ in 0..15 {
        let start = Instant::now();
        let mut sum = 0.0f32;
        for _ in 0..20 {
            let p = layer.pack();
            for r in 0..rows {
                sum += layer.scales()[r] * p.row_dot(r, &x);
            }
        }
        best_old = best_old.min(start.elapsed().as_secs_f64());
        black_box(sum);
        let start = Instant::now();
        let mut sum = 0.0f32;
        for _ in 0..20 {
            sum += black_box(layer.forward(&x)).iter().sum::<f32>();
        }
        best_new = best_new.min(start.elapsed().as_secs_f64());
        black_box(sum);
    }

    let old: Vec<f32> = {
        let p = layer.pack();
        (0..rows)
            .map(|r| layer.scales()[r] * p.row_dot(r, &x))
            .collect()
    };
    let new = layer.forward(&x);
    let eq = old
        .iter()
        .zip(&new)
        .filter(|(a, b)| a.to_bits() == b.to_bits())
        .count();

    println!("forward old {:.4} ms", best_old * 1000.0 / 20.0);
    println!("forward new {:.4} ms", best_new * 1000.0 / 20.0);
    println!("speed       {:.2}x", best_old / best_new);
    println!("bit-equal   {eq}/{rows}");
}
