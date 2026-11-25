use super::train_step;
use crate::dense;
use crate::plane::fused::forward_dot;
use crate::quant::Ste;

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

#[allow(clippy::too_many_arguments)]
fn old_train_step(
    latent: &mut [f32],
    scale: &mut [f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    target: &[f32],
    lr: f32,
    ste: Ste,
) -> f32 {
    let qdot = forward_dot(latent, scale, rows, cols, x);
    let y: Vec<f32> = qdot.iter().zip(&*scale).map(|(&d, &s)| s * d).collect();
    let mut dy = vec![0.0f32; rows];
    let mut loss = 0.0f32;
    for (r, slot) in dy.iter_mut().enumerate() {
        let diff = y[r] - target[r];
        *slot = diff;
        loss += 0.5 * diff * diff;
    }
    dense::sgd_update(latent, x, rows, cols, &dy, scale, lr, ste);
    for r in 0..rows {
        scale[r] = (scale[r] - lr * dy[r] * qdot[r]).max(1e-4);
    }
    loss / rows as f32
}

#[test]
fn matches_forward_dot_plus_sgd_update_bitwise() {
    for (rows, cols) in [
        (1usize, 1usize),
        (2, 7),
        (5, 9),
        (9, 65),
        (11, 130),
        (4, 576),
    ] {
        for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
            let latent = sample(rows * cols, 401 + (rows * 100 + cols) as u64);
            let x = sample(cols, 409 + rows as u64);
            let target = sample(rows, 419 + cols as u64);
            let mut scale: Vec<f32> = sample(rows, 421 + cols as u64)
                .into_iter()
                .map(|v| v.abs() + 0.01)
                .collect();
            if rows > 1 {
                scale[1] = -0.5;
            }

            let mut old_latent = latent.clone();
            let mut old_scale = scale.clone();
            let mut new_latent = latent;
            let mut new_scale = scale;
            for step in 0..3 {
                let old_loss = old_train_step(
                    &mut old_latent,
                    &mut old_scale,
                    rows,
                    cols,
                    &x,
                    &target,
                    0.05,
                    ste,
                );
                let new_loss = train_step(
                    &mut new_latent,
                    &mut new_scale,
                    rows,
                    cols,
                    &x,
                    &target,
                    0.05,
                    ste,
                );
                assert_eq!(
                    new_loss.to_bits(),
                    old_loss.to_bits(),
                    "{ste:?} {rows}x{cols} step {step} loss"
                );
                assert_eq!(
                    new_latent, old_latent,
                    "{ste:?} {rows}x{cols} step {step} latent"
                );
                assert_eq!(
                    new_scale, old_scale,
                    "{ste:?} {rows}x{cols} step {step} scale"
                );
            }
        }
    }
}
