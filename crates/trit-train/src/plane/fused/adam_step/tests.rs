use super::{AdamParams, adam_step_scalar};
use crate::plane::PlaneMatrix;
use crate::quant::Ste;
fn sample(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state % 2001) as f32 - 1000.0) / 500.0
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn old_update(
    latent: &mut [f32],
    dx: &mut [f32],
    qdot: &mut [f32],
    x: &[f32],
    dy: &[f32],
    scale: &[f32],
    m: &mut [f32],
    v: &mut [f32],
    params: AdamParams,
    ste: Ste,
) {
    let rows = dy.len();
    let cols = x.len();
    let planes = PlaneMatrix::from_latent(latent, scale, rows, cols);
    planes.input_grad_scalar(dy, scale, dx);
    for (r, slot) in qdot.iter_mut().enumerate() {
        *slot = planes.row_dot_scalar(r, x);
    }
    for r in 0..rows {
        let s = scale[r];
        let coeff = dy[r] * s;
        let base = r * cols;
        for (c, &xc) in x.iter().enumerate() {
            let idx = base + c;
            let grad = ste.backward(latent[idx], s, coeff * xc);
            m[idx] = params.beta1 * m[idx] + (1.0 - params.beta1) * grad;
            v[idx] = params.beta2 * v[idx] + (1.0 - params.beta2) * grad * grad;
            let m_hat = m[idx] / params.bias1;
            let v_hat = v[idx] / params.bias2;
            latent[idx] -= params.lr * m_hat / (v_hat.sqrt() + params.eps);
        }
    }
}

#[test]
fn scalar_adam_step_matches_old_path() {
    for (rows, cols) in [
        (1usize, 1usize),
        (2, 7),
        (5, 9),
        (9, 65),
        (11, 130),
        (4, 576),
    ] {
        for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
            let latent = sample(rows * cols, 301 + (rows * 100 + cols) as u64);
            let x = sample(cols, 303 + rows as u64);
            let dy = sample(rows, 307 + cols as u64);
            let scale: Vec<f32> = sample(rows, 309 + cols as u64)
                .into_iter()
                .map(|v| v.abs() + 0.01)
                .collect();
            let (mut fast_latent, mut slow_latent) = (latent.clone(), latent);
            let mut fast_m = vec![0.0f32; rows * cols];
            let mut fast_v = vec![0.0f32; rows * cols];
            let mut slow_m = fast_m.clone();
            let mut slow_v = fast_v.clone();
            for step in 1..=3 {
                let params = AdamParams::new(
                    0.01,
                    0.9,
                    0.999,
                    1e-8,
                    1.0 - 0.9f32.powi(step),
                    1.0 - 0.999f32.powi(step),
                );
                let (dx_fast, qdot_fast) = adam_step_scalar(
                    &mut fast_latent,
                    &scale,
                    rows,
                    cols,
                    &x,
                    &dy,
                    &mut fast_m,
                    &mut fast_v,
                    params,
                    ste,
                );
                let mut dx_slow = vec![0.0f32; cols];
                let mut qdot_slow = vec![0.0f32; rows];
                old_update(
                    &mut slow_latent,
                    &mut dx_slow,
                    &mut qdot_slow,
                    &x,
                    &dy,
                    &scale,
                    &mut slow_m,
                    &mut slow_v,
                    params,
                    ste,
                );
                assert_eq!(dx_fast, dx_slow, "scalar {ste:?} {rows}x{cols} dX");
                assert_eq!(qdot_fast, qdot_slow, "scalar {ste:?} {rows}x{cols} qdot");
                assert_eq!(
                    fast_latent, slow_latent,
                    "scalar {ste:?} {rows}x{cols} latent"
                );
                assert_eq!(fast_m, slow_m, "scalar {ste:?} {rows}x{cols} m");
                assert_eq!(fast_v, slow_v, "scalar {ste:?} {rows}x{cols} v");
            }
        }
    }
}
