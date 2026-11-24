use super::*;

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

#[test]
fn full_adam_step_matches_scalar_reference() {
    for (rows, cols) in [(1usize, 1usize), (2, 7), (5, 9), (9, 65), (11, 130)] {
        for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
            let mut state = 0x9e37_79b9_7f4a_7c15u64;
            let init = |_, _| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state % 2001) as f32 - 1000.0) / 1000.0
            };
            let mut fast = TernaryLinear::new(rows, cols, init, ste, 0.05, 0.5);
            let mut slow = fast.clone();
            let mut fast_latent = Adam::new(rows * cols, 0.01);
            let mut slow_latent = Adam::new(rows * cols, 0.01);
            let mut fast_scale = Adam::new(rows, 0.01);
            let mut slow_scale = Adam::new(rows, 0.01);
            let mut data = 0x243f_6a88_85a3_08d3u64;
            for step in 0..3 {
                let x = sample(cols, data ^ step as u64);
                let dy = sample(rows, data ^ (step as u64 + 0x1000));
                data ^= data << 13;
                let got =
                    fast.train_step_with_grad_adam(&x, &dy, &mut fast_latent, &mut fast_scale);
                let want = slow.train_step_with_grad_adam_scalar(
                    &x,
                    &dy,
                    &mut slow_latent,
                    &mut slow_scale,
                );
                assert_close(&got, &want, "dX", (rows, cols));
                assert_close(fast.latent(), slow.latent(), "latent", (rows, cols));
                assert_close(fast.scales(), slow.scales(), "scale", (rows, cols));
                assert_close(&fast_latent.m, &slow_latent.m, "m", (rows, cols));
                assert_close(&fast_latent.v, &slow_latent.v, "v", (rows, cols));
                assert!((fast_latent.bias1 - slow_latent.bias1).abs() < 1e-7);
                assert!((fast_latent.bias2 - slow_latent.bias2).abs() < 1e-7);
                assert_eq!(fast.quantized(), slow.quantized());
            }
        }
    }
}
