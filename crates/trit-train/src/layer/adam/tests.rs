use super::support::adam_update;
use super::*;
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

fn scalar_reference(
    latent: &mut [f32],
    x: &[f32],
    rows: usize,
    dy: &[f32],
    scale: &[f32],
    ste: Ste,
    opt: &mut Adam,
) {
    let cols = x.len();
    for r in 0..rows {
        let coeff = dy[r] * scale[r];
        for (c, &xc) in x.iter().take(cols).enumerate() {
            let idx = r * cols + c;
            let grad = ste.backward(latent[idx], scale[r], coeff * xc);
            latent[idx] = opt.apply(idx, grad, latent[idx]);
        }
    }
}

fn assert_close(got: &[f32], want: &[f32], what: &str, shape: (usize, usize)) {
    for (i, (a, b)) in got.iter().zip(want).enumerate() {
        assert!(
            (a - b).abs() <= 1e-4 * (1.0 + b.abs()),
            "{what} {shape:?} idx={i}: fast={a} slow={b}"
        );
    }
}

#[test]
fn adam_update_matches_scalar_for_all_ste_variants() {
    for (rows, cols) in [(2usize, 7usize), (5, 9), (9, 65), (11, 130)] {
        for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
            let latent = sample(rows * cols, 301 + (rows * 100 + cols) as u64);
            let x = sample(cols, 303 + rows as u64);
            let dy = sample(rows, 307 + cols as u64);
            let scale: Vec<f32> = sample(rows, 309 + cols as u64)
                .into_iter()
                .map(|v| v.abs() + 0.01)
                .collect();
            let mut fast = latent.clone();
            let mut slow = latent;
            let mut fast_opt = Adam::new(rows * cols, 0.01);
            let mut slow_opt = Adam::new(rows * cols, 0.01);
            for step in 0..3 {
                fast_opt.begin();
                slow_opt.begin();
                adam_update(&mut fast, &x, rows, cols, &dy, &scale, ste, &mut fast_opt);
                scalar_reference(&mut slow, &x, rows, &dy, &scale, ste, &mut slow_opt);
                assert_close(&fast, &slow, "latent", (rows, cols));
                assert_close(&fast_opt.m, &slow_opt.m, "m", (rows, cols));
                assert_close(&fast_opt.v, &slow_opt.v, "v", (rows, cols));
                assert!((fast_opt.bias1 - slow_opt.bias1).abs() < 1e-7);
                assert!((fast_opt.bias2 - slow_opt.bias2).abs() < 1e-7);
                assert_eq!(fast_opt.step(), step + 1);
            }
        }
    }
}

#[test]
fn adam_update_is_noop_for_zero_lr() {
    let latent = sample(5 * 65, 411);
    let x = sample(65, 423);
    let dy = sample(5, 427);
    let scale = vec![0.5f32; 5];
    let mut got = latent.clone();
    let mut opt = Adam::new(5 * 65, 0.0);
    opt.begin();
    adam_update(&mut got, &x, 5, 65, &dy, &scale, Ste::Soft, &mut opt);
    assert_eq!(got, latent);
}

mod closed_form;
mod fused;
