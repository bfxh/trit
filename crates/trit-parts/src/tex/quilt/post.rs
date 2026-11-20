//! quilt 的后处理:盒式模糊(去光照低频)与亮度回贴(平面/分位)。
use super::super::util::luma_of;

/// 逐通道盒式模糊（去光照用的低频）。
pub(crate) fn box_blur_rgb(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0.0f32; src.len()];
    let n = (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut sum = 0.0f32;
                for k in 0..=2 * r {
                    let xx = (x + k).saturating_sub(r).min(w - 1);
                    sum += src[(y * w + xx) * 3 + c];
                }
                tmp[(y * w + x) * 3 + c] = sum / n;
            }
        }
    }
    let mut out = vec![0.0f32; src.len()];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut sum = 0.0f32;
                for k in 0..=2 * r {
                    let yy = (y + k).saturating_sub(r).min(h - 1);
                    sum += tmp[(yy * w + x) * 3 + c];
                }
                out[(y * w + x) * 3 + c] = sum / n;
            }
        }
    }
    out
}

/// 拼接完成后的亮度回贴:`relight_plane` 回贴源图拟合的线性平面,否则 `relight`
/// 走亮度分位匹配(加性)。内容自 `synth.rs` 原样迁移,行为逐位不变。
pub(crate) fn apply_post(
    mut out: Vec<f32>,
    out_w: usize,
    out_h: usize,
    src_orig: &[f32],
    plane: [[f32; 3]; 3],
    relight: bool,
    relight_plane: bool,
) -> Vec<f32> {
    if relight_plane {
        // **旧行为**：回贴源图拟合的线性平面。结构保得住，但对比度只回一小部分（§6.80/§6.81）。
        for y in 0..out_h {
            let v = y as f32 / out_h.max(1) as f32;
            for x in 0..out_w {
                let u = x as f32 / out_w.max(1) as f32;
                let o = (y * out_w + x) * 3;
                for c in 0..3 {
                    let g = plane[c][0] + plane[c][1] * u + plane[c][2] * v;
                    out[o + c] = (out[o + c] + g).clamp(0.0, 1.0);
                }
            }
        }
    } else if relight {
        // **亮度分位匹配（加性）**：把输出亮度分位对到源亮度分布，位移量加到 RGB 上。
        // 逐通道独立映射也能补回对比度，但会改色相与梯度（实测 coh 0.865→0.470、hf1 +94%）⇒ 用加性保结构。
        let mut src_luma: Vec<f32> = src_orig.chunks(3).map(luma_of).collect();
        src_luma.sort_by(f32::total_cmp);
        let mut out_sorted: Vec<f32> = out.chunks(3).map(luma_of).collect();
        out_sorted.sort_by(f32::total_cmp);
        let (ns, no) = (src_luma.len(), out_sorted.len());
        if ns > 1 && no > 1 {
            for i in 0..out.len() / 3 {
                let o = i * 3;
                let y0 = luma_of(&out[o..o + 3]);
                let rank = out_sorted.partition_point(|v| *v < y0).min(no - 1);
                let q = rank as f32 / (no - 1) as f32;
                let t = (q * (ns - 1) as f32).round() as usize;
                let delta = src_luma[t.min(ns - 1)] - y0;
                for c in 0..3 {
                    out[o + c] = (out[o + c] + delta).clamp(0.0, 1.0);
                }
            }
        }
    }
    out
}
