//! CPU 泼溅光栅：投影 → 视深排序（远→近 painter）→ 逐像素 alpha 合成 → RGB8 帧。
//!
//! 排序口径：纯 splat 场景用视深排序近似（与 3DGS 训练渲染一致）；
//! mesh↔splat 的射线深度 + CDF 软截断在 C0b 接入（见 depth-contract/README）。
//!
//! **只用帧尾截断，不再按屏幕 σ / 各向异性比例硬剔 splat**：巨型或细长
//! splat 在近景、地面和背景里是合法几何；按尺寸硬剔会在画面上留下黑
//! 洞（scan-wasteland / wasteland-orbit 实测 PSNR 掉 8–10 dB）。唯一
//! 的剔除条件是：这颗高斯在整幅帧内的峰值 α 都低于 1/255，即它本来
//! 就不会产生任何可见像素——数学上等价于逐像素 α 截断，不会改变输出。
//!
//! **背景**：CLI 默认纯黑，与 Brush / gaussian-splatting 的 eval 一致；
//! 训练时背景已经烘进 splat，若再把 GT 边界均值垫在底下，半透明像素
//! 会被整体提亮（synthetic-orbit 实测 30.2→33.3 dB）。

use crate::Frame;

use crate::camera::Intrinsics;
use crate::math::Mat4;
use crate::ply::RawGaussian;
use crate::splat::{ScreenGaussian, project};

use depth::render_depth;
use tail::tail_visible;

mod depth;
mod tail;

/// 帧尾可见性阈值：峰值 α 低于 1/255 的高斯对任何像素都不可见。
pub const TAIL_ALPHA_CUTOFF: f32 = 1.0 / 255.0;

/// 渲染一帧（只要 RGB）。`gaussians` 顺序无关（内部按视深排序）。
/// `min_scales`：每 splat 的 Mip-Splatting 3D 滤波下限（None=关闭）。
pub fn render(
    gaussians: &[RawGaussian],
    c2w: &Mat4,
    intr: &Intrinsics,
    background: [f32; 3],
    min_scales: Option<&[f32]>,
) -> Frame {
    render_impl(gaussians, c2w, intr, background, min_scales, false).0
}

/// 渲染一帧并返回相机空间深度（alpha 加权期望视深；未覆盖像素为 +∞）。
/// 深度单位与 `ScreenGaussian::depth` 一致（相机前方为正）。
pub fn render_with_depth(
    gaussians: &[RawGaussian],
    c2w: &Mat4,
    intr: &Intrinsics,
    background: [f32; 3],
    min_scales: Option<&[f32]>,
) -> (Frame, Vec<f32>) {
    render_impl(gaussians, c2w, intr, background, min_scales, true)
}

#[allow(clippy::too_many_arguments)]
fn render_impl(
    gaussians: &[RawGaussian],
    c2w: &Mat4,
    intr: &Intrinsics,
    background: [f32; 3],
    min_scales: Option<&[f32]>,
    want_depth: bool,
) -> (Frame, Vec<f32>) {
    let mut splats: Vec<ScreenGaussian> = gaussians
        .iter()
        .enumerate()
        .filter_map(|(idx, g)| project(g, c2w, intr, min_scales.and_then(|m| m.get(idx).copied())))
        .filter(|s| tail_visible(s, intr))
        .collect();
    splats.sort_by(|a, b| {
        b.depth
            .partial_cmp(&a.depth)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // SPLAT_DUMP=<路径>：把排序后的每 splat 中间量导出（u,v,depth,inv_a,inv_b,
    // inv_c,alpha,r,g,b），供 numpy 合成实验台逐级对拍，消灭转写错误。
    if let Ok(dump_path) = std::env::var("SPLAT_DUMP") {
        let mut rows = Vec::with_capacity(splats.len() * 10);
        for s in &splats {
            rows.extend_from_slice(&[
                s.u, s.v, s.depth, s.inv_a, s.inv_b, s.inv_c, s.alpha, s.color[0], s.color[1],
                s.color[2],
            ]);
        }
        let bytes: Vec<u8> = rows.iter().flat_map(|v| v.to_le_bytes()).collect();
        match std::fs::write(&dump_path, &bytes) {
            Ok(_) => println!("SPLAT_DUMP 写出 {} 颗 → {dump_path}", splats.len()),
            Err(e) => eprintln!("SPLAT_DUMP 写出失败: {e}"),
        }
    }

    let w = intr.width as i32;
    let h = intr.height as i32;
    let pixels = (intr.width as usize) * (intr.height as usize);
    let mut canvas = Vec::with_capacity(pixels * 3);
    for _ in 0..pixels {
        canvas.extend_from_slice(&[
            background[0].clamp(0.0, 1.0),
            background[1].clamp(0.0, 1.0),
            background[2].clamp(0.0, 1.0),
        ]);
    }

    for s in &splats {
        // Σ⁻¹=[[a,b],[b,c]] ⇒ σ_x=sqrt(c/det_inv), σ_y=sqrt(a/det_inv)
        let det_inv = s.inv_a * s.inv_c - s.inv_b * s.inv_b;
        if det_inv <= 0.0 {
            continue;
        }
        let sigma_x = (s.inv_c / det_inv).sqrt();
        let sigma_y = (s.inv_a / det_inv).sqrt();
        // bbox 半宽按 Brush compute_bbox_extent 口径：α 自适应
        // ex = √(2·ln(α·255))·σ —— α=1 时 3.33σ，α=0.1 时 2.55σ（低 α 裁尾更紧）
        let power_threshold = (s.alpha * 255.0).max(1e-6).ln();
        let ex = (2.0 * power_threshold * sigma_x * sigma_x).sqrt();
        let ey = (2.0 * power_threshold * sigma_y * sigma_y).sqrt();
        let x0 = (s.u - ex).floor() as i32;
        let x1 = (s.u + ex).ceil() as i32;
        let y0 = (s.v - ey).floor() as i32;
        let y1 = (s.v + ey).ceil() as i32;

        for py in y0.max(0)..=(y1.min(h - 1)) {
            let dy = py as f32 + 0.5 - s.v;
            for px in x0.max(0)..=(x1.min(w - 1)) {
                let dx = px as f32 + 0.5 - s.u;
                let power =
                    -0.5 * (dx * dx * s.inv_a + 2.0 * dx * dy * s.inv_b + dy * dy * s.inv_c);
                if power > 0.0 {
                    continue; // 数值噪声
                }
                let alpha_px = (s.alpha * power.exp()).min(0.999);
                if alpha_px < TAIL_ALPHA_CUTOFF {
                    continue;
                }
                let idx = py as usize * intr.width as usize + px as usize;
                let o = idx * 3;
                for c in 0..3 {
                    canvas[o + c] = s.color[c] * alpha_px + canvas[o + c] * (1.0 - alpha_px);
                }
            }
        }
    }
    let rgb = canvas
        .into_iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let frame = Frame {
        width: intr.width,
        height: intr.height,
        rgb,
    };
    let depth = if want_depth {
        render_depth(&splats, intr)
    } else {
        Vec::new()
    };
    (frame, depth)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/depth.rs"]
mod depth_tests;
