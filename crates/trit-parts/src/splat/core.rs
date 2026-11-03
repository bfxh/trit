use std::collections::HashMap;

use crate::fuse::View;
use crate::geo::Camera;

/// 体素去重累加器：(位置和, 颜色和, 计数)。
type Acc = ([f64; 3], [f64; 3], u32);

/// 一帧拟合输入：(相机, 宽, 高, GT 颜色, 前景掩码)。
pub type FitView = (Camera, usize, usize, Vec<f32>, Vec<u32>);

/// 拟合用的预收集权重：(相机, 宽, 高, GT, 掩码, 权重)。
type Pre = (Camera, usize, usize, Vec<f32>, Vec<u32>, SplatWeights);

/// 一个各向同性高斯。
#[derive(Clone, Debug)]
pub struct Splat {
    pub pos: [f32; 3],
    /// 世界系半径（米）。
    pub scale: f32,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// 从多视角（id 图 + 深度 + RGB）建高斯：按世界体素去重，位置/颜色取均值。
/// `cell` 越大精度越低；`scale_k` 是半径相对体素的比例。
pub fn build(views: &[View], rgbs: &[Vec<f32>], cell: f32, scale_k: f32) -> Vec<Splat> {
    let mut acc: HashMap<(i32, i32, i32), Acc> = HashMap::new();
    let inv = 1.0 / cell.max(1e-5);
    for (v, rgb) in views.iter().zip(rgbs) {
        if rgb.len() != v.w * v.h * 3 {
            continue;
        }
        for i in 0..v.ids.len().min(v.w * v.h) {
            let d = v.depth[i];
            if v.ids[i] == 0 || !(d.is_finite() && d > 0.0 && d < 50.0) {
                continue;
            }
            let (u, vv) = (i % v.w, i / v.w);
            let p = v.cam.unproject(u as f64, vv as f64, d);
            let key = (
                (p[0] * inv).floor() as i32,
                (p[1] * inv).floor() as i32,
                (p[2] * inv).floor() as i32,
            );
            let e = acc.entry(key).or_insert(([0.0; 3], [0.0; 3], 0));
            for a in 0..3 {
                e.0[a] += p[a] as f64;
                e.1[a] += rgb[i * 3 + a] as f64;
            }
            e.2 += 1;
        }
    }
    acc.values()
        .map(|(p, c, n)| {
            let n = (*n).max(1) as f64;
            Splat {
                pos: [0, 1, 2].map(|a| (p[a] / n) as f32),
                scale: cell * scale_k,
                color: [0, 1, 2].map(|a| (c[a] / n) as f32),
                alpha: 0.6,
            }
        })
        .collect()
}

/// 每个高斯在每个像素上的权重（已按近到远合成、含透射率），供渲染与梯度复用。
pub struct SplatWeights {
    /// (高斯序号, 像素序号, 权重)。
    pub entries: Vec<(u32, u32, f32)>,
    /// 每像素被覆盖的比例（0..1）。
    pub coverage: Vec<f32>,
}

/// 收集权重：几何/alpha 决定，与颜色无关 ⇒ 颜色可以精确求梯度。
pub fn collect(splats: &[Splat], cam: &Camera, w: usize, h: usize) -> SplatWeights {
    let fx = cam.k[0][0];
    struct Item {
        idx: u32,
        depth: f64,
        u: f64,
        v: f64,
        sigma: f64,
        alpha: f32,
    }
    let mut items: Vec<Item> = Vec::with_capacity(splats.len());
    for (i, s) in splats.iter().enumerate() {
        let Some((u, v, d)) = cam.project(s.pos) else {
            continue;
        };
        if u < -2.0 || v < -2.0 || u >= w as f64 + 2.0 || v >= h as f64 + 2.0 {
            continue;
        }
        let sigma = (fx * s.scale as f64 / d).max(0.35);
        items.push(Item {
            idx: i as u32,
            depth: d,
            u,
            v,
            sigma,
            alpha: s.alpha,
        });
    }
    items.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    let mut coverage = vec![0.0f32; w * h];
    let mut entries = Vec::new();
    for it in &items {
        let r = (it.sigma * 2.5).ceil() as i64;
        let cx = it.u.round() as i64;
        let cy = it.v.round() as i64;
        for y in (cy - r).max(0)..=(cy + r).min(h as i64 - 1) {
            for x in (cx - r).max(0)..=(cx + r).min(w as i64 - 1) {
                let dx = (x as f64 - it.u) / it.sigma;
                let dy = (y as f64 - it.v) / it.sigma;
                let g = (-0.5 * (dx * dx + dy * dy)).exp();
                if g < 1e-3 {
                    continue;
                }
                let p = y as usize * w + x as usize;
                let a = (it.alpha as f64 * g) as f32;
                let wgt = a * (1.0 - coverage[p]).max(0.0);
                coverage[p] += wgt;
                entries.push((it.idx, p as u32, wgt));
            }
        }
    }
    SplatWeights { entries, coverage }
}

/// 用已收集的权重渲染（`bg` 用来填没覆盖到的像素）。
pub fn render_from(
    splats: &[Splat],
    cw: &SplatWeights,
    w: usize,
    h: usize,
    bg: [f32; 3],
) -> Vec<f32> {
    let mut img = vec![0.0f32; w * h * 3];
    for &(i, p, wt) in &cw.entries {
        let c = splats[i as usize].color;
        for k in 0..3 {
            img[p as usize * 3 + k] += wt * c[k];
        }
    }
    for p in 0..w * h {
        let rest = (1.0 - cw.coverage[p]).max(0.0);
        for k in 0..3 {
            img[p * 3 + k] += rest * bg[k];
        }
    }
    img
}

/// 渲染一帧：近到远 alpha 合成（EWA 的简化：屏幕空间各向同性高斯）。
pub fn render(splats: &[Splat], cam: &Camera, w: usize, h: usize, bg: [f32; 3]) -> Vec<f32> {
    let cw = collect(splats, cam, w, h);
    render_from(splats, &cw, w, h, bg)
}

/// 只拟合**颜色**（位置/尺度/alpha 固定）：颜色对渲染是线性的，
/// `∂L/∂c_i = Σ_p 2·(img_p − gt_p)·w_{i,p}` 是精确梯度。返回每轮留出 PSNR（由调用方算）。
pub fn fit_colors(splats: &mut [Splat], views: &[FitView], iters: usize, lr: f32, bg: [f32; 3]) {
    if splats.is_empty() || views.is_empty() {
        return;
    }
    let pre: Vec<Pre> = views
        .iter()
        .map(|(cam, w, h, gt, ids)| {
            (
                cam.clone(),
                *w,
                *h,
                gt.clone(),
                ids.clone(),
                collect(splats, cam, *w, *h),
            )
        })
        .collect();
    for _ in 0..iters {
        let mut grad = vec![[0.0f64; 3]; splats.len()];
        let mut cnt = vec![0u32; splats.len()];
        for (cam, w, h, gt, ids, cw) in &pre {
            let img = render_from(splats, cw, *w, *h, bg);
            for &(i, p, wt) in &cw.entries {
                if wt <= 0.0 || ids[p as usize] == 0 {
                    continue; // 只拟合前景像素（和 PSNR 口径一致）
                }
                for k in 0..3 {
                    let r = f64::from(img[p as usize * 3 + k] - gt[p as usize * 3 + k]);
                    grad[i as usize][k] += 2.0 * r * f64::from(wt);
                }
                cnt[i as usize] += 1;
            }
            let _ = (cam, w, h);
        }
        for (i, s) in splats.iter_mut().enumerate() {
            if cnt[i] == 0 {
                continue;
            }
            let n = f64::from(cnt[i]) as f64;
            for (k, c) in s.color.iter_mut().enumerate() {
                let g = (grad[i][k] / n) as f32;
                *c = (*c - lr * g).clamp(0.0, 1.0);
            }
        }
    }
}

/// PSNR（输入 0..1）。
pub fn psnr(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len(), "PSNR 输入长度不一致");
    let mse: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let d = f64::from(*x - *y);
            d * d
        })
        .sum::<f64>()
        / a.len().max(1) as f64;
    if mse <= 1e-12 {
        return f64::INFINITY;
    }
    10.0 * (1.0 / mse).log10()
}
