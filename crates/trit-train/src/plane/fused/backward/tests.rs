use super::backward;
use crate::plane::PlaneMatrix;

fn sample(rows: usize, cols: usize, seed: u64) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let latent = (0..rows * cols)
        .map(|i| (((i as u64 + seed) * 0x9E37_79B9) as f32 * 0.000_013).sin() * 0.9)
        .collect();
    let scale = (0..rows)
        .map(|r| 0.21 + 0.13 * ((r as f32 + seed as f32) * 0.017).cos().abs())
        .collect();
    let x = (0..cols)
        .map(|c| ((c as f32 + seed as f32) * 0.011).cos())
        .collect();
    let dy = (0..rows)
        .map(|r| ((r as f32 + seed as f32) * 0.023).sin())
        .collect();
    (latent, scale, x, dy)
}

#[test]
fn matches_pack_then_input_grad_and_row_dot() {
    for (rows, cols) in [
        (1usize, 1usize),
        (4, 7),
        (5, 9),
        (6, 63),
        (8, 130),
        (4, 576),
        (3, 9),
    ] {
        let (mut latent, mut scale, x, dy) = sample(rows, cols, 7 + rows as u64);
        if rows == 3 {
            latent[0] = 0.0;
            latent[4] = 1e-30;
            scale[0] = 0.0;
            scale[1] = -1.0;
        }
        let planes = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
        let mut want_dx = vec![0.0f32; cols];
        planes.input_grad(&dy, &scale, &mut want_dx);
        let want_q: Vec<f32> = (0..rows).map(|r| planes.row_dot(r, &x)).collect();
        let (dx, qdot) = backward(&latent, &scale, rows, cols, &x, &dy);
        for c in 0..cols {
            assert_eq!(dx[c].to_bits(), want_dx[c].to_bits(), "dX c={c}");
        }
        for r in 0..rows {
            assert_eq!(qdot[r].to_bits(), want_q[r].to_bits(), "qdot r={r}");
        }
    }
}
