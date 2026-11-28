//! 深度缓冲：近→远前向合成，累计 alpha 加权期望视深。

use crate::camera::Intrinsics;
use crate::splat::ScreenGaussian;

/// 近→远前向合成，累计 alpha 加权期望视深；覆盖不足的像素为 +∞。
pub(super) fn render_depth(splats: &[ScreenGaussian], intr: &Intrinsics) -> Vec<f32> {
    let pixels = (intr.width as usize) * (intr.height as usize);
    let w = intr.width as i32;
    let h = intr.height as i32;
    let mut trans = vec![1.0f32; pixels];
    let mut sum = vec![0.0f32; pixels];
    let mut weight = vec![0.0f32; pixels];
    for s in splats.iter().rev() {
        let det_inv = s.inv_a * s.inv_c - s.inv_b * s.inv_b;
        if det_inv <= 0.0 {
            continue;
        }
        let sigma_x = (s.inv_c / det_inv).sqrt();
        let sigma_y = (s.inv_a / det_inv).sqrt();
        let power_threshold = (s.alpha * 255.0).max(1e-6).ln();
        let ex = (2.0 * power_threshold * sigma_x * sigma_x).sqrt();
        let ey = (2.0 * power_threshold * sigma_y * sigma_y).sqrt();
        let x0 = (s.u - ex).floor() as i32;
        let x1 = (s.u + ex).ceil() as i32;
        let y0 = (s.v - ey).floor() as i32;
        let y1 = (s.v + ey).ceil() as i32;
        if s.alpha < 1.0 / 255.0 {
            continue;
        }
        for py in y0.max(0)..=(y1.min(h - 1)) {
            let dy = py as f32 + 0.5 - s.v;
            for px in x0.max(0)..=(x1.min(w - 1)) {
                let dx = px as f32 + 0.5 - s.u;
                let power =
                    -0.5 * (dx * dx * s.inv_a + 2.0 * dx * dy * s.inv_b + dy * dy * s.inv_c);
                if power > 0.0 {
                    continue;
                }
                let alpha_px = (s.alpha * power.exp()).min(0.999);
                if alpha_px < 1.0 / 255.0 {
                    continue;
                }
                let idx = py as usize * intr.width as usize + px as usize;
                let wi = alpha_px * trans[idx];
                sum[idx] += s.depth * wi;
                weight[idx] += wi;
                trans[idx] *= (1.0 - alpha_px).max(0.0);
            }
        }
    }
    sum.iter()
        .zip(weight.iter())
        .map(|(sum, weight)| {
            if *weight > 1e-6 {
                sum / weight
            } else {
                f32::INFINITY
            }
        })
        .collect()
}
