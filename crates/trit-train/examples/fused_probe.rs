//! 复算 `fused::forward_dot` 与 `pack()+row_dot` 的差距。
//!
//! `cargo run -p trit-train --release --example fused_probe`

use std::hint::black_box;
use std::time::Instant;

use trit_train::plane::PlaneMatrix;
use trit_train::plane::fused::forward_dot;

fn main() {
    let rows = 4096;
    let cols = 576;
    let mut latent = vec![0.0f32; rows * cols];
    let mut scale = vec![0.0f32; rows];
    let mut x = vec![0.0f32; cols];
    for r in 0..rows {
        scale[r] = 0.3 + 0.2 * ((r % 17) as f32 / 17.0);
        for c in 0..cols {
            latent[r * cols + c] = ((r * 31 + c * 7) as f32 * 0.017).sin() * 0.9;
        }
    }
    for (c, xc) in x.iter_mut().enumerate() {
        *xc = ((c * 13) as f32 * 0.011).cos();
    }

    let mut best_pack = f64::INFINITY;
    let mut best_pack_dot = f64::INFINITY;
    let mut best_fused = f64::INFINITY;
    for _ in 0..15 {
        let start = Instant::now();
        for _ in 0..20 {
            black_box(PlaneMatrix::from_latent(&latent, &scale, rows, cols));
        }
        best_pack = best_pack.min(start.elapsed().as_secs_f64());
        let start = Instant::now();
        let mut sum = 0.0f32;
        for _ in 0..20 {
            let p = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
            for r in 0..rows {
                sum += p.row_dot(r, &x);
            }
        }
        best_pack_dot = best_pack_dot.min(start.elapsed().as_secs_f64());
        black_box(sum);
        let start = Instant::now();
        let mut sum = 0.0f32;
        for _ in 0..20 {
            let v = forward_dot(&latent, &scale, rows, cols, &x);
            sum += black_box(v).iter().sum::<f32>();
        }
        best_fused = best_fused.min(start.elapsed().as_secs_f64());
        black_box(sum);
    }

    let planes = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
    let want: Vec<f32> = (0..rows).map(|r| planes.row_dot(r, &x)).collect();
    let got = forward_dot(&latent, &scale, rows, cols, &x);
    let eq = want
        .iter()
        .zip(&got)
        .filter(|(a, b)| a.to_bits() == b.to_bits())
        .count();

    println!("pack        {:.4} ms", best_pack * 1000.0 / 20.0);
    println!("pack+dot    {:.4} ms", best_pack_dot * 1000.0 / 20.0);
    println!("fused       {:.4} ms", best_fused * 1000.0 / 20.0);
    println!("speed       {:.2}x", best_pack_dot / best_fused);
    println!("bit-equal   {eq}/{rows}");
}
