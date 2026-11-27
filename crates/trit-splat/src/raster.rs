//! CPU 泼溅光栅：投影 → 退化 splat 闸门 → 视深排序（远→近 painter）→ 逐像素
//! alpha 合成 → RGB8 帧。
//!
//! 排序口径：纯 splat 场景用视深排序近似（与 3DGS 训练渲染一致）；
//! mesh↔splat 的射线深度 + CDF 软截断在 C0b 接入（见 depth-contract/README）。
//!
//! **退化 splat 闸门**：MCM 训练会产生针状巨型 splat（实测 scan-wasteland
//! 有 266m×14cm、opacity 0.999 的针贴着相机轨道），其屏幕 σ 覆盖全屏时
//! 会以近乎满 opacity 盖掉整幅画面（Brush 的 eval 不受影响是因为其内部
//! 处理；对 PLY 消费端这是必须自防的病理）。闸门：屏幕空间 σ 超过
//! [`MAX_SCREEN_SIGMA_RATIO`]×画面最长边的高斯不参与合成。

use crate::camera::Intrinsics;
use crate::math::Mat4;
use crate::ply::RawGaussian;
use crate::splat::{project, ScreenGaussian};
use crate::Frame;

/// 屏幕 σ 闸门比例：σ > ratio × max(w, h) 的高斯视为退化 splat。
pub const MAX_SCREEN_SIGMA_RATIO: f32 = 0.3;

fn screen_sigma(s: &ScreenGaussian) -> Option<f32> {
    let det_inv = s.inv_a * s.inv_c - s.inv_b * s.inv_b;
    if det_inv <= 0.0 {
        return None;
    }
    Some((s.inv_c / det_inv).sqrt().max((s.inv_a / det_inv).sqrt()))
}

/// 渲染一帧。`gaussians` 顺序无关（内部按视深排序）。
pub fn render(
    gaussians: &[RawGaussian],
    c2w: &Mat4,
    intr: &Intrinsics,
    background: [f32; 3],
) -> Frame {
    let sigma_cap = MAX_SCREEN_SIGMA_RATIO * intr.width.max(intr.height) as f32;
    let mut splats: Vec<ScreenGaussian> = gaussians
        .iter()
        .filter_map(|g| project(g, c2w, intr))
        .filter(|s| screen_sigma(s).is_none_or(|sigma| sigma < sigma_cap))
        .collect();
    splats.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));

    let mut frame = Frame::new(intr.width, intr.height, background);
    let w = intr.width as i32;
    let h = intr.height as i32;

    for s in &splats {
        // Σ⁻¹=[[a,b],[b,c]] ⇒ σ_x=sqrt(c/det_inv), σ_y=sqrt(a/det_inv)
        let det_inv = s.inv_a * s.inv_c - s.inv_b * s.inv_b;
        if det_inv <= 0.0 {
            continue;
        }
        let sigma_x = (s.inv_c / det_inv).sqrt();
        let sigma_y = (s.inv_a / det_inv).sqrt();
        let x0 = (s.u - 3.0 * sigma_x).floor() as i32;
        let x1 = (s.u + 3.0 * sigma_x).ceil() as i32;
        let y0 = (s.v - 3.0 * sigma_y).floor() as i32;
        let y1 = (s.v + 3.0 * sigma_y).ceil() as i32;
        if s.alpha < 1.0 / 255.0 {
            continue;
        }

        for py in y0.max(0)..=(y1.min(h - 1)) {
            let dy = py as f32 + 0.5 - s.v;
            for px in x0.max(0)..=(x1.min(w - 1)) {
                let dx = px as f32 + 0.5 - s.u;
                let power = -0.5
                    * (dx * dx * s.inv_a + 2.0 * dx * dy * s.inv_b + dy * dy * s.inv_c);
                if power > 0.0 {
                    continue; // 数值噪声
                }
                let alpha_px = s.alpha * power.exp();
                if alpha_px < 1.0 / 255.0 {
                    continue;
                }
                let o = ((py as u32) * intr.width + px as u32) as usize * 3;
                for c in 0..3 {
                    let canvas = frame.rgb[o + c] as f32 / 255.0;
                    let out = s.color[c] * alpha_px + canvas * (1.0 - alpha_px);
                    frame.rgb[o + c] = (out.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
    }
    frame
}

/// `--bg auto` 用：GT 帧边界 4px 环带的均值作背景色（对拍用，注明口径）。
pub fn border_mean(rgb: &[u8], width: u32, height: u32, band: u32) -> [f32; 3] {
    let w = width as i32;
    let h = height as i32;
    let band = band as i32;
    let (mut r, mut g, mut b, mut n) = (0.0f64, 0.0f64, 0.0f64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let edge = x < band || x >= w - band || y < band || y >= h - band;
            if !edge {
                continue;
            }
            let o = ((y as u32) * width + x as u32) as usize * 3;
            r += rgb[o] as f64;
            g += rgb[o + 1] as f64;
            b += rgb[o + 2] as f64;
            n += 1;
        }
    }
    if n == 0 {
        return [0.4, 0.4, 0.4];
    }
    let n = n as f64;
    [(r / n) as f32 / 255.0, (g / n) as f32 / 255.0, (b / n) as f32 / 255.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Intrinsics;
    use crate::ply::RawGaussian;

    const SH0_REF: f32 = 0.282_094_791_773_878_14;

    /// 纯色 DC 系数：color = f_dc·C0 + 0.5，⇒ f_dc = (target−0.5)/C0。
    fn pure_dc(target: [f32; 3]) -> [f32; 3] {
        target.map(|t| (t - 0.5) / SH0_REF)
    }

    fn gaussian(pos: [f32; 3], color_dc: [f32; 3], scale: f32) -> RawGaussian {
        RawGaussian {
            position: pos,
            f_dc: color_dc,
            f_rest: vec![],
            opacity_logit: 1e8,
            scale_log: [scale.ln(); 3],
            rot_xyzw: [0.0, 0.0, 0.0, 1.0],
        }
    }

    fn cam_at(z: f32) -> Mat4 {
        let mut m = Mat4::identity();
        m.m[2][3] = z;
        m
    }

    fn intr_small() -> Intrinsics {
        Intrinsics { fl_x: 50.0, fl_y: 50.0, cx: 16.0, cy: 12.0, width: 32, height: 24 }
    }

    #[test]
    fn single_red_gaussian_paints_center() {
        // 高斯在光轴上 (0,0,0)，相机在 z=5：中心像素应为纯红
        let g = gaussian([0.0, 0.0, 0.0], pure_dc([1.0, 0.0, 0.0]), 0.2);
        let frame = render(&[g], &cam_at(5.0), &intr_small(), [0.0, 0.0, 0.0]);
        let o = (12 * 32 + 16) * 3;
        assert!(frame.rgb[o] > 200, "中心应偏红: {}", frame.rgb[o]);
        assert!(frame.rgb[o + 1] < 40, "中心绿应低: {}", frame.rgb[o + 1]);
        // 角落保持背景
        let corner = 0;
        assert!(frame.rgb[corner] < 5 && frame.rgb[corner + 2] < 5);
    }

    #[test]
    fn nearer_splat_wins_at_overlap() {
        // 同轴两颗：近的绿 (z=3) 压过远的红 (z=0)，相机 z=5
        let far = gaussian([0.0, 0.0, 0.0], pure_dc([1.0, 0.0, 0.0]), 0.25);
        let near = gaussian([0.0, 0.0, 2.0], pure_dc([0.0, 1.0, 0.0]), 0.25);
        let frame = render(&[far, near], &cam_at(5.0), &intr_small(), [0.0, 0.0, 0.0]);
        let o = (12 * 32 + 16) * 3;
        assert!(frame.rgb[o + 1] > 200, "近的绿应胜出: {}", frame.rgb[o + 1]);
        assert!(frame.rgb[o] < 40, "远的红应被盖: {}", frame.rgb[o]);
    }

    #[test]
    fn border_mean_of_uniform_image_is_its_color() {
        let w = 8u32;
        let h = 8u32;
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        for px in rgb.chunks_exact_mut(3) {
            px.copy_from_slice(&[200, 100, 50]);
        }
        let mean = border_mean(&rgb, w, h, 2);
        assert!((mean[0] - 200.0 / 255.0).abs() < 1e-4);
        assert!((mean[2] - 50.0 / 255.0).abs() < 1e-4);
    }

    #[test]
    fn degenerate_giant_splat_does_not_wash_frame() {
        // 复现 scan-wasteland 病理：266m 针状 splat（opacity≈1）贴着相机，
        // 屏幕 σ 全屏覆盖 → 闸门应剔除它，不干净色盖掉小 splat。
        let giant = gaussian([0.0, 0.0, 4.0], pure_dc([1.0, 0.0, 0.0]), 266.0);
        let small = gaussian([0.0, 0.0, 0.0], pure_dc([0.0, 0.0, 1.0]), 0.2);
        let frame = render(&[giant, small], &cam_at(5.0), &intr_small(), [0.0, 0.0, 0.0]);
        let o = (12 * 32 + 16) * 3;
        assert!(
            frame.rgb[o + 2] > 100,
            "小蓝 splat 应可见: r={} g={} b={}",
            frame.rgb[o],
            frame.rgb[o + 1],
            frame.rgb[o + 2]
        );
        assert!(frame.rgb[o] < 100, "巨红 splat 应被闸门剔除: r={}", frame.rgb[o]);
    }
}
