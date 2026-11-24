use super::super::support::adam_update;
use super::assert_close;
use super::sample;
use crate::layer::{Adam, TernaryLinear};
use crate::quant::Ste;

#[test]
fn full_adam_step_matches_old_two_pass_bitwise() {
    for (rows, cols) in [
        (1usize, 1usize),
        (2, 7),
        (5, 9),
        (9, 65),
        (11, 130),
        (4, 576),
    ] {
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

                slow_latent.begin();
                slow_scale.begin();
                let (want, qdot) = crate::plane::fused::backward::backward(
                    &slow.latent,
                    &slow.scale,
                    rows,
                    cols,
                    &x,
                    &dy,
                );
                adam_update(
                    &mut slow.latent,
                    &x,
                    rows,
                    cols,
                    &dy,
                    &slow.scale,
                    ste,
                    &mut slow_latent,
                );
                for (r, &dy_r) in dy.iter().enumerate() {
                    slow.scale[r] = slow_scale.apply(r, dy_r * qdot[r], slow.scale[r]).max(1e-4);
                }

                assert_eq!(got, want, "old two-pass {ste:?} {rows}x{cols} dX");
                assert_eq!(
                    fast.latent, slow.latent,
                    "old two-pass {ste:?} {rows}x{cols} latent"
                );
                assert_eq!(
                    fast.scale, slow.scale,
                    "old two-pass {ste:?} {rows}x{cols} scale"
                );
                assert_eq!(
                    fast_latent.m, slow_latent.m,
                    "old two-pass {ste:?} {rows}x{cols} m"
                );
                assert_eq!(
                    fast_latent.v, slow_latent.v,
                    "old two-pass {ste:?} {rows}x{cols} v"
                );
            }
        }
    }
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
