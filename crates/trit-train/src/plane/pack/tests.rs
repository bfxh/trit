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

#[test]
fn from_latent_matches_quantize_with_scale() {
    use crate::quant::quantize_with_scale;
    for (rows, cols) in [(1usize, 1usize), (3, 7), (5, 64), (4, 65), (7, 130)] {
        let latent = sample_f32(rows * cols, 71 + (rows * 100 + cols) as u64);
        let scale: Vec<f32> = sample_f32(rows, 83 + cols as u64)
            .into_iter()
            .map(|v| v.abs() + 0.01)
            .collect();
        let m = PlaneMatrix::from_latent(&latent, &scale, rows, cols);
        let q: Vec<i8> = (0..rows)
            .flat_map(|r| quantize_with_scale(&latent[r * cols..(r + 1) * cols], scale[r]))
            .collect();
        assert_eq!(m.q(), q.as_slice());
        assert_eq!(m, PlaneMatrix::from_quantized(q, rows, cols));
    }
}

#[test]
fn from_latent_matches_scalar_at_half_boundaries() {
    let latent = [
        0.5, -0.5, 1.5, -1.5, 0.499, -0.499, 2.0, -2.0, 0.5001, -0.5001, 1.0, -1.0, 0.0, -0.0, 3.5,
        -3.5,
    ];
    let scale = [1.0f32];
    let fast = PlaneMatrix::from_latent(&latent, &scale, 1, latent.len());
    let slow = PlaneMatrix::from_latent_scalar(&latent, &scale, 1, latent.len());
    assert_eq!(fast, slow);
    assert_eq!(
        fast.q(),
        &[1, -1, 1, -1, 0, 0, 1, -1, 1, -1, 1, -1, 0, 0, 1, -1]
    );
}
