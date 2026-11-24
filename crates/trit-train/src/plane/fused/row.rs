//! 单行量化点积：`Σ q·x`，供 `train_step` 的逐行融合使用。
use super::has_avx2;

pub(crate) fn forward_dot_row(latent: &[f32], scale: f32, cols: usize, x: &[f32]) -> f32 {
    assert_eq!(latent.len(), cols, "latent 行长度必须是 cols");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    #[cfg(target_arch = "x86_64")]
    if cols >= 8 && has_avx2() {
        // SAFETY: 长度已断言；内核按 8 列整块与尾部访问。
        return unsafe { super::simd::row(latent, scale, x, cols) };
    }
    forward_dot_row_scalar(latent, scale, cols, x)
}

fn forward_dot_row_scalar(latent: &[f32], scale: f32, cols: usize, x: &[f32]) -> f32 {
    let s = scale.max(f32::MIN_POSITIVE);
    let mut sum = 0.0f32;
    for (&w, &xc) in latent.iter().zip(x).take(cols) {
        let q = (w / s).round().clamp(-1.0, 1.0);
        if q > 0.0 {
            sum += xc;
        } else if q < 0.0 {
            sum -= xc;
        }
    }
    sum
}
