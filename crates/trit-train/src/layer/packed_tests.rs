//! 位平面训练步与标量参考的对拍测试。

use super::TernaryLinear;
use crate::quant::Ste;

fn sample_x(seed: &mut u64, cols: usize) -> Vec<f32> {
    (0..cols)
        .map(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            ((*seed % 2001) as f32 - 1000.0) / 1000.0
        })
        .collect()
}

#[test]
fn packed_forward_matches_scalar_forward() {
    for (rows, cols) in [(2usize, 3usize), (5, 9), (7, 64), (9, 65), (11, 130)] {
        let mut state = 0x517c_c1b7_2722_0a95u64;
        let layer = TernaryLinear::new(
            rows,
            cols,
            |_, _| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state % 2001) as f32 - 1000.0) / 1000.0
            },
            Ste::Soft,
            0.05,
            0.5,
        );
        let mut data = 0x9e37_79b9_7f4a_7c15u64;
        let x = sample_x(&mut data, cols);
        let fast = layer.forward(&x);
        let slow = layer.forward_scalar(&x);
        for (r, (a, b)) in fast.iter().zip(&slow).enumerate() {
            assert!(
                (a - b).abs() <= 1e-4 * (1.0 + b.abs()),
                "rows={rows} cols={cols} r={r}: fast={a} slow={b}"
            );
        }
    }
}

#[test]
fn packed_train_step_matches_scalar_one_step() {
    for (rows, cols) in [(2usize, 3usize), (5, 9), (9, 65)] {
        for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
            let mut state = 0x243f_6a88_85a3_08d3u64;
            let mut a = TernaryLinear::new(
                rows,
                cols,
                |_, _| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    ((state % 100) as f32 - 50.0) * 0.01
                },
                ste,
                0.05,
                0.5,
            );
            let mut b = a.clone();
            let mut data = 0x1319_8a2e_0370_7344u64;
            let x = sample_x(&mut data, cols);
            let target: Vec<f32> = sample_x(&mut data, rows);
            let loss_a = a.train_step(&x, &target);
            let loss_b = b.train_step_scalar(&x, &target);
            assert!(
                (loss_a - loss_b).abs() <= 1e-4 * (1.0 + loss_b.abs()),
                "{ste:?} {rows}x{cols}: packed={loss_a} scalar={loss_b}"
            );
            for (i, (wa, wb)) in a.latent().iter().zip(b.latent()).enumerate() {
                assert!(
                    (wa - wb).abs() <= 1e-4 * (1.0 + wb.abs()),
                    "{ste:?} {rows}x{cols} idx={i}: packed={wa} scalar={wb}"
                );
            }
            for (sa, sb) in a.scales().iter().zip(b.scales()) {
                assert!((sa - sb).abs() <= 1e-4 * (1.0 + sb.abs()));
            }
        }
    }
}
