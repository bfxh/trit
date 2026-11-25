//! `fused::row` 在 `q` 阈值附近的逐位对拍：直接对旧 `pack` 路径。

use super::super::forward_dot;
use crate::plane::PlaneMatrix;

#[test]
fn matches_pack_at_half_threshold() {
    let rows = 2;
    let cols = 16;
    let scale = [0.5f32, 0.5];
    let mut latent = vec![0.0f32; rows * cols];
    for j in 0..8 {
        latent[j] = f32::from_bits((0.25f32.to_bits() as i64 + j as i64 - 4) as u32);
        latent[cols + j] = f32::from_bits((-0.25f32).to_bits() + 1 - j as u32);
    }
    let x: Vec<f32> = (0..cols).map(|c| (c as f32 * 0.17).cos()).collect();
    let planes = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
    let want: Vec<f32> = (0..rows).map(|r| planes.row_dot(r, &x)).collect();
    let got = forward_dot(&latent, &scale, rows, cols, &x);
    for (r, (&a, &b)) in want.iter().zip(&got).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "half-threshold mismatch r={r}");
    }
}
