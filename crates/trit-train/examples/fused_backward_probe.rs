//! 复算 `fused::backward` 与 `pack()+input_grad+row_dot` 的差距。
//!
//! `cargo run -p trit-train --release --example fused_backward_probe`

use std::hint::black_box;
use std::time::Instant;

use trit_train::plane::PlaneMatrix;
use trit_train::plane::fused::backward::backward;

fn main() {
    let rows = 4096;
    let cols = 576;
    let mut latent = vec![0.0f32; rows * cols];
    let mut scale = vec![0.0f32; rows];
    let x: Vec<f32> = (0..cols).map(|c| ((c * 13) as f32 * 0.011).cos()).collect();
    let dy: Vec<f32> = (0..rows).map(|r| ((r * 17) as f32 * 0.007).sin()).collect();
    for r in 0..rows {
        scale[r] = 0.3 + 0.2 * ((r % 17) as f32 / 17.0);
        for c in 0..cols {
            latent[r * cols + c] = ((r * 31 + c * 7) as f32 * 0.017).sin() * 0.9;
        }
    }

    let mut best_old = f64::INFINITY;
    let mut best_new = f64::INFINITY;
    for _ in 0..15 {
        let start = Instant::now();
        let mut sum = 0.0f32;
        for _ in 0..20 {
            let p = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
            let mut dx = vec![0.0f32; cols];
            p.input_grad(&dy, &scale, &mut dx);
            for r in 0..rows {
                sum += p.row_dot(r, &x);
            }
            black_box(dx);
        }
        best_old = best_old.min(start.elapsed().as_secs_f64());
        black_box(sum);
        let start = Instant::now();
        let mut sum = 0.0f32;
        for _ in 0..20 {
            let (dx, qdot) = backward(&latent, &scale, rows, cols, &x, &dy);
            sum += qdot.iter().sum::<f32>();
            black_box(dx);
        }
        best_new = best_new.min(start.elapsed().as_secs_f64());
        black_box(sum);
    }

    let p = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
    let mut want_dx = vec![0.0f32; cols];
    p.input_grad(&dy, &scale, &mut want_dx);
    let want_q: Vec<f32> = (0..rows).map(|r| p.row_dot(r, &x)).collect();
    let (dx, qdot) = backward(&latent, &scale, rows, cols, &x, &dy);
    let dx_eq = dx
        .iter()
        .zip(&want_dx)
        .filter(|(a, b)| a.to_bits() == b.to_bits())
        .count();
    let q_eq = qdot
        .iter()
        .zip(&want_q)
        .filter(|(a, b)| a.to_bits() == b.to_bits())
        .count();

    println!("old         {:.4} ms", best_old * 1000.0 / 20.0);
    println!("new         {:.4} ms", best_new * 1000.0 / 20.0);
    println!("speed       {:.2}x", best_old / best_new);
    println!("dx equal    {dx_eq}/{cols}");
    println!("qdot equal  {q_eq}/{rows}");
}
