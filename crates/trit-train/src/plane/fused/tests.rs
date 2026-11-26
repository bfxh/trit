use super::forward_dot;
use crate::plane::PlaneMatrix;
fn sample(rows: usize, cols: usize, seed: u64) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let latent = (0..rows * cols)
        .map(|i| (((i as u64 + seed) * 0x9E37_79B9) as f32 * 0.000_013).sin() * 0.9)
        .collect();
    let scale = (0..rows)
        .map(|r| 0.21 + 0.13 * ((r as f32 + seed as f32) * 0.017).cos().abs())
        .collect();
    let x = (0..cols)
        .map(|c| ((c as f32 + seed as f32) * 0.011).cos())
        .collect();
    (latent, scale, x)
}

#[test]
fn matches_pack_then_row_dot() {
    for (rows, cols) in [
        (1usize, 1usize),
        (4, 7),
        (5, 9),
        (6, 63),
        (8, 130),
        (4, 576),
        (3, 9),
    ] {
        let (mut latent, mut scale, x) = sample(rows, cols, 7 + rows as u64);
        if rows == 3 {
            latent[0] = 0.0;
            latent[4] = 1e-30;
            scale[0] = 0.0;
            scale[1] = -1.0;
        }
        let planes = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
        let want: Vec<f32> = (0..rows).map(|r| planes.row_dot(r, &x)).collect();
        let got = forward_dot(&latent, &scale, rows, cols, &x);
        for (r, (&a, &b)) in want.iter().zip(&got).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "fused mismatch r={r}");
        }
    }
}
mod threshold;
