//! 帧尾截断：只剔除整幅帧都不可见的高斯，不用屏幕尺寸/各向异性硬闸门。

use crate::camera::Intrinsics;
use crate::splat::ScreenGaussian;

use super::TAIL_ALPHA_CUTOFF;

fn sigma_max(s: &ScreenGaussian) -> Option<f32> {
    let det_inv = s.inv_a * s.inv_c - s.inv_b * s.inv_b;
    if det_inv <= 0.0 {
        return None;
    }
    // 2×2 对称矩阵的最大特征值（不能只看 max(inv_a, inv_c)：非对角项
    // 会把长轴转到屏幕斜方向，只看对角会低估 σ、误剔可见的斜向巨尾）。
    let half = 0.5 * (s.inv_a + s.inv_c);
    let rad = (0.25 * (s.inv_a - s.inv_c) * (s.inv_a - s.inv_c) + s.inv_b * s.inv_b).sqrt();
    Some(((half + rad) / det_inv).sqrt())
}

/// 高斯中心到帧矩形的最短距离（在帧内为 0）。
fn frame_distance(s: &ScreenGaussian, intr: &Intrinsics) -> f32 {
    let w = intr.width as f32;
    let h = intr.height as f32;
    let du = if s.u < 0.0 {
        -s.u
    } else if s.u > w {
        s.u - w
    } else {
        0.0
    };
    let dv = if s.v < 0.0 {
        -s.v
    } else if s.v > h {
        s.v - h
    } else {
        0.0
    };
    du.hypot(dv)
}

/// 帧尾截断：若高斯在最近帧像素上的峰值 α 仍低于落盘阈值，则整颗不可见。
pub(super) fn tail_visible(s: &ScreenGaussian, intr: &Intrinsics) -> bool {
    let Some(sigma) = sigma_max(s) else {
        return false;
    };
    if s.alpha < TAIL_ALPHA_CUTOFF {
        return false;
    }
    let d = frame_distance(s, intr);
    if d <= 0.0 {
        return true;
    }
    s.alpha * (-0.5 * d * d / (sigma * sigma)).exp() >= TAIL_ALPHA_CUTOFF
}
