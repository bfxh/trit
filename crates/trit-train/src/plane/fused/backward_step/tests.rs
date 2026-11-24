use super::backward_step;
use crate::plane::PlaneMatrix;
use crate::quant::Ste;
#[path = "tests/scalar.rs"]
mod scalar;
fn sample(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state % 2001) as f32 - 1000.0) / 600.0
        })
        .collect()
}
#[test]
fn matches_backward_plus_sgd_update_for_all_ste_variants() {
    for (rows, cols) in [
        (1usize, 1usize),
        (4, 7),
        (5, 9),
        (6, 63),
        (8, 130),
        (3, 9),
        (4, 576),
    ] {
        for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
            let latent = sample(rows * cols, 301 + (rows * 100 + cols) as u64);
            let x = sample(cols, 307 + rows as u64);
            let dy = sample(rows, 311 + cols as u64);
            let mut scale: Vec<f32> = sample(rows, 313 + cols as u64)
                .into_iter()
                .map(|v| v.abs() + 0.01)
                .collect();
            if rows > 0 {
                scale[0] = 0.0;
            }
            if rows > 1 {
                scale[1] = -0.5;
            }

            let planes = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
            let mut dx_old = vec![0.0f32; cols];
            planes.input_grad(&dy, &scale, &mut dx_old);
            let qdot: Vec<f32> = (0..rows).map(|r| planes.row_dot(r, &x)).collect();
            let mut old_latent = latent.clone();
            let mut old_scale = scale.clone();
            crate::dense::sgd_update(&mut old_latent, &x, rows, cols, &dy, &scale, 0.05, ste);
            for (r, slot) in old_scale.iter_mut().enumerate() {
                *slot = (scale[r] - 0.05 * dy[r] * qdot[r]).max(1e-4);
            }

            let mut new_latent = latent;
            let mut new_scale = scale;
            let dx_new = backward_step(
                &mut new_latent,
                &mut new_scale,
                rows,
                cols,
                &x,
                &dy,
                0.05,
                ste,
            );

            assert_eq!(dx_new, dx_old, "{ste:?} {rows}x{cols} dX");
            assert_eq!(new_latent, old_latent, "{ste:?} {rows}x{cols} latent");
            assert_eq!(new_scale, old_scale, "{ste:?} {rows}x{cols} scale");
        }
    }
}
