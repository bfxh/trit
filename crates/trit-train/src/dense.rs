//! 逐行 `dW` 外积更新：`w[r,c] -= lr · STE(w[r,c], scale[r], dy[r]·scale[r]·x[c])`。
//!
//! 这是 `dW` 的融合更新，不是位平面运算；`dX` 的位平面内核在
//! [`crate::plane::PlaneMatrix`]。SIMD 路径只在 x86_64 + AVX2 + 列数 ≥ 8 时启用，
//! 其余走标量参考。

#[cfg(test)]
use crate::quant::Ste;
/// 对整层 latent 做一次 SGD 外积更新。
///
/// 计时/调用口径：`dy`、`scale` 长度是 `rows`，`x` 长度是 `cols`，
/// `latent` 是行主序 `rows * cols`；`scale[r]` 是更新前的值。
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn sgd_update(
    latent: &mut [f32],
    x: &[f32],
    rows: usize,
    cols: usize,
    dy: &[f32],
    scale: &[f32],
    lr: f32,
    ste: Ste,
) {
    assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    assert_eq!(dy.len(), rows, "dy 长度必须是 rows");
    assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
    for r in 0..rows {
        let coeff = dy[r] * scale[r];
        sgd_update_row(
            &mut latent[r * cols..(r + 1) * cols],
            x,
            coeff,
            lr,
            scale[r],
            ste,
        );
    }
}

mod row;
pub(crate) use row::sgd_update_row;

#[cfg(target_arch = "x86_64")]
fn has_avx2() -> bool {
    std::arch::is_x86_feature_detected!("avx2")
}

#[cfg(not(target_arch = "x86_64"))]
#[allow(dead_code)]
fn has_avx2() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_f32(len: usize, seed: u64) -> Vec<f32> {
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
        lr: f32,
        ste: Ste,
    ) {
        let cols = x.len();
        for (r, (&dy_r, &s)) in dy.iter().zip(scale).take(rows).enumerate() {
            let coeff = dy_r * s;
            for (c, &xc) in x.iter().take(cols).enumerate() {
                let idx = r * cols + c;
                latent[idx] -= lr * ste.backward(latent[idx], s, coeff * xc);
            }
        }
    }

    #[test]
    fn sgd_update_matches_scalar_for_all_ste_variants() {
        for (rows, cols) in [(2usize, 7usize), (5, 9), (9, 65), (11, 130)] {
            for ste in [Ste::Identity, Ste::Clipped { ratio: 1.0 }, Ste::Soft] {
                let latent = sample_f32(rows * cols, 101 + (rows * 100 + cols) as u64);
                let x = sample_f32(cols, 103 + rows as u64);
                let dy = sample_f32(rows, 107 + cols as u64);
                let scale: Vec<f32> = sample_f32(rows, 109 + cols as u64)
                    .into_iter()
                    .map(|v| v.abs() + 0.01)
                    .collect();
                let mut fast = latent.clone();
                let mut slow = latent;
                sgd_update(&mut fast, &x, rows, cols, &dy, &scale, 0.05, ste);
                scalar_reference(&mut slow, &x, rows, &dy, &scale, 0.05, ste);
                for (i, (a, b)) in fast.iter().zip(&slow).enumerate() {
                    assert!(
                        (a - b).abs() <= 1e-4 * (1.0 + b.abs()),
                        "{ste:?} {rows}x{cols} idx={i}: fast={a} slow={b}"
                    );
                }
            }
        }
    }

    #[test]
    fn sgd_update_is_noop_for_zero_lr() {
        let latent = sample_f32(5 * 65, 211);
        let x = sample_f32(65, 223);
        let dy = sample_f32(5, 227);
        let scale = vec![0.5f32; 5];
        let mut got = latent.clone();
        sgd_update(&mut got, &x, 5, 65, &dy, &scale, 0.0, Ste::Soft);
        assert_eq!(got, latent);
    }
}
