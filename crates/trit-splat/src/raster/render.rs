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

use crate::Frame;

use crate::camera::Intrinsics;
use crate::math::Mat4;
use crate::ply::RawGaussian;
use crate::splat::{ScreenGaussian, project};

/// 屏幕 σ 闸门比例：σ > ratio × max(w, h) 的高斯视为退化 splat。
pub const MAX_SCREEN_SIGMA_RATIO: f32 = 0.6;

fn screen_sigma(s: &ScreenGaussian) -> Option<f32> {
    let det_inv = s.inv_a * s.inv_c - s.inv_b * s.inv_b;
    if det_inv <= 0.0 {
        return None;
    }
    Some((s.inv_c / det_inv).sqrt().max((s.inv_a / det_inv).sqrt()))
}

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
    let sigma_cap = std::env::var("SPLAT_SIGMA_CAP")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or_else(|| MAX_SCREEN_SIGMA_RATIO * intr.width.max(intr.height) as f32);
    // 各向异性比率闸门：σ_max/σ_min 超过此值的针状退化 splat 剔除
    // （MCM 针实测 ratio≈1900；合法远景/地面 splat ratio 通常 <10）。
    let aniso_cap = std::env::var("SPLAT_ANISO_CAP")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(100.0);
    let mut splats: Vec<ScreenGaussian> = gaussians
        .iter()
        .enumerate()
        .filter_map(|(idx, g)| project(g, c2w, intr, min_scales.and_then(|m| m.get(idx).copied())))
        .filter(|s| {
            let Some(sigma) = screen_sigma(s) else {
                return false;
            };
            if sigma >= sigma_cap {
                return false;
            }
            let det_inv = s.inv_a * s.inv_c - s.inv_b * s.inv_b;
            if det_inv <= 0.0 {
                return false;
            }
            let sigma_min = (1.0 / det_inv).sqrt();
            sigma / sigma_min.max(1e-6) < aniso_cap
        })
        .collect();
    splats.sort_by(|a, b| {
        b.depth
            .partial_cmp(&a.depth)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // SPLAT_DUMP=<路径>：把排序后的每 splat 中间量导出（u,v,depth,inv_a,inv_b,
    // inv_c,alpha,r,g,b），供 numpy 合成实验台逐级对拍，消灭转写错误。
    if let Ok(dump_path) = std::env::var("SPLAT_DUMP") {
        let mut rows = Vec::with_capacity(splats.len() * 9);
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
        // bbox 半宽按 Brush compute_bbox_extent 口径：α 自适应
        // ex = √(2·ln(α·255))·σ —— α=1 时 3.33σ，α=0.1 时 2.55σ（低 α 裁尾更紧）
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
                    continue; // 数值噪声
                }
                let alpha_px = s.alpha * power.exp();
                if alpha_px < 1.0 / 255.0 {
                    continue;
                }
                let idx = py as usize * intr.width as usize + px as usize;
                let o = idx * 3;
                for c in 0..3 {
                    let canvas = frame.rgb[o + c] as f32 / 255.0;
                    let out = s.color[c] * alpha_px + canvas * (1.0 - alpha_px);
                    frame.rgb[o + c] = (out.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
    }
    let depth = if want_depth {
        render_depth(&splats, intr)
    } else {
        Vec::new()
    };
    (frame, depth)
}

/// 近→远前向合成，累计 alpha 加权期望视深；覆盖不足的像素为 +∞。
fn render_depth(splats: &[ScreenGaussian], intr: &Intrinsics) -> Vec<f32> {
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
                let alpha_px = s.alpha * power.exp();
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/depth.rs"]
mod depth_tests;
