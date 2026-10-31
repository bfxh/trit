//! 高斯喷溅（f32）：多视角表面采样 → 各向同性高斯 → EWA 近似的 2D 投影 + 前到后 alpha 合成。
//!
//! 这一档**故意不走三值**：位置/尺度/颜色是连续量，三值化只会损失精度；
//! 三值留给「学」的部分。精度由采样格 `cell` 与高斯半径系数控制，形成精度阶梯。
//! 本轮不做梯度拟合（颜色直接取源像素）；拟合是下一步。

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

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        Camera {
            k: [[100.0, 0.0, 50.0], [0.0, 100.0, 50.0], [0.0, 0.0, 1.0]],
            t_cw: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    #[test]
    fn single_splat_renders_at_projection() {
        let c = cam();
        // 世界 (0,0,-2) 投到像素 (50,50)
        let s = Splat {
            pos: [0.0, 0.0, -2.0],
            scale: 0.05,
            color: [1.0, 0.0, 0.0],
            alpha: 1.0,
        };
        let img = render(&[s], &c, 100, 100, [0.0, 0.0, 0.0]);
        let p = 50 * 100 + 50;
        assert!(img[p * 3] > 0.5, "中心应为红：{}", img[p * 3]);
        assert!(img[p * 3 + 1] < 0.1);
    }

    #[test]
    fn near_splat_occludes_far_one() {
        let c = cam();
        let near = Splat {
            pos: [0.0, 0.0, -1.0],
            scale: 0.05,
            color: [0.0, 1.0, 0.0],
            alpha: 1.0,
        };
        let far = Splat {
            pos: [0.0, 0.0, -3.0],
            scale: 0.05,
            color: [0.0, 0.0, 1.0],
            alpha: 1.0,
        };
        let img = render(&[far, near], &c, 100, 100, [0.0, 0.0, 0.0]);
        let p = 50 * 100 + 50;
        assert!(img[p * 3 + 1] > img[p * 3 + 2], "近处绿色应盖住远处蓝色");
    }

    fn loss_of(splats: &[Splat], views: &[FitView]) -> f64 {
        let mut l = 0.0;
        for (cam, w, h, gt, ids) in views {
            let c = build_cache(splats, cam, *w, *h);
            let img = render_cached(splats, &c, *w, *h, [0.0, 0.0, 0.0]);
            for p in 0..*w * *h {
                if ids[p] == 0 {
                    continue;
                }
                for k in 0..3 {
                    let d = f64::from(img[p * 3 + k] - gt[p * 3 + k]);
                    l += d * d;
                }
            }
        }
        l
    }

    #[test]
    fn jacobian_of_projection_matches_numeric() {
        let c = cam();
        let s = Splat {
            pos: [0.3, -0.2, -2.0],
            scale: 0.05,
            color: [1.0; 3],
            alpha: 0.8,
        };
        let base = c.project(s.pos).unwrap();
        let eps = 1e-3; // f32 位置：差分步要够大
        for a in 0..3 {
            let mut q = s.pos;
            q[a] += eps as f32;
            let up = c.project(q).unwrap();
            for (name, num, ana) in [
                ("u", (up.0 - base.0) / eps, {
                    // 解析：fx·(R[a][0]·D + xb·R[a][2])/D²
                    let t = &c.t_cw;
                    let (xb, _yb) = (
                        t[0][0] * q[0] as f64 + t[1][0] * q[1] as f64 + t[2][0] * q[2] as f64,
                        t[0][1] * q[0] as f64 + t[1][1] * q[1] as f64 + t[2][1] * q[2] as f64,
                    );
                    let d = base.2;
                    c.k[0][0] * (t[a][0] * d + xb * t[a][2]) / (d * d)
                }),
                ("v", (up.1 - base.1) / eps, {
                    let t = &c.t_cw;
                    let (_xb, yb) = (
                        t[0][0] * q[0] as f64 + t[1][0] * q[1] as f64 + t[2][0] * q[2] as f64,
                        t[0][1] * q[0] as f64 + t[1][1] * q[1] as f64 + t[2][1] * q[2] as f64,
                    );
                    let d = base.2;
                    c.k[1][1] * (-t[a][1] * d + (-yb) * t[a][2]) / (d * d)
                }),
                ("d", (up.2 - base.2) / eps, -c.t_cw[a][2]),
            ] {
                assert!(
                    (num - ana).abs() < 5e-3 * ana.abs().max(1.0),
                    "axis {a} {name}: 数值 {num} vs 解析 {ana}"
                );
            }
        }
        let _ = s;
    }

    #[test]
    fn analytic_geometry_gradients_match_finite_difference() {
        let c = cam();
        let (w, h) = (100usize, 100usize);
        let s0 = Splat {
            pos: [0.0, 0.0, -2.0],
            scale: 0.05,
            color: [1.0, 0.2, 0.2],
            alpha: 0.8,
        };
        let gt = [0.1f32, 0.9, 0.2].repeat(w * h);
        let ids = vec![1u32; w * h];
        let views = vec![(c.clone(), w, h, gt, ids)];
        let (_gc, gp, gs, ga, cnt) = gradients(std::slice::from_ref(&s0), &views, [0.0, 0.0, 0.0]);
        assert!(cnt[0] > 0, "高斯应覆盖到像素");
        let _ = cnt;
        let eps = 1e-4;
        // 位置 x
        for (a, &ana) in gp[0].iter().enumerate() {
            let mut up = s0.clone();
            up.pos[a] += eps as f32;
            let mut dn = s0.clone();
            dn.pos[a] -= eps as f32;
            let num = (loss_of(&[up], &views) - loss_of(&[dn], &views)) / (2.0 * eps);
            assert!(
                (num - ana).abs() < 5e-2 * ana.abs().max(1e-3) + 1e-3,
                "pos[{a}]: 数值 {num:.6} vs 解析 {ana:.6}"
            );
        }
        // 尺度
        let mut up = s0.clone();
        up.scale += eps as f32;
        let mut dn = s0.clone();
        dn.scale -= eps as f32;
        let num = (loss_of(&[up], &views) - loss_of(&[dn], &views)) / (2.0 * eps);
        let ana = gs[0];
        assert!(
            (num - ana).abs() < 5e-2 * ana.abs().max(1e-3) + 1e-3,
            "scale: 数值 {num:.6} vs 解析 {ana:.6}"
        );
        // alpha
        let mut up = s0.clone();
        up.alpha += eps as f32;
        let mut dn = s0.clone();
        dn.alpha -= eps as f32;
        let num = (loss_of(&[up], &views) - loss_of(&[dn], &views)) / (2.0 * eps);
        let ana = ga[0];
        assert!(
            (num - ana).abs() < 5e-2 * ana.abs().max(1e-3) + 1e-3,
            "alpha: 数值 {num:.6} vs 解析 {ana:.6}"
        );
    }

    #[test]
    fn two_splat_transmittance_chain_matches_numeric() {
        let c = cam();
        let (w, h) = (100usize, 100usize);
        let near = Splat {
            pos: [0.0, 0.0, -1.6],
            scale: 0.06,
            color: [0.9, 0.1, 0.1],
            alpha: 0.7,
        };
        let far = Splat {
            pos: [0.05, 0.0, -2.2],
            scale: 0.06,
            color: [0.1, 0.1, 0.9],
            alpha: 0.7,
        };
        let gt = [0.3f32, 0.7, 0.3].repeat(w * h);
        let ids = vec![1u32; w * h];
        let views = vec![(c.clone(), w, h, gt, ids)];
        let (_gc, gp, _gs, _ga, cnt) =
            gradients(&[near.clone(), far.clone()], &views, [0.0, 0.0, 0.0]);
        let eps = 1e-3f32;
        for i in 0..2 {
            for a in [0usize, 2] {
                let mut up = [near.clone(), far.clone()];
                up[i].pos[a] += eps;
                let mut dn = [near.clone(), far.clone()];
                dn[i].pos[a] -= eps;
                let num = (loss_of(&up, &views) - loss_of(&dn, &views)) / (2.0 * f64::from(eps));
                let ana = gp[i][a];
                assert!(cnt[i] > 0);
                assert!(
                    (num - ana).abs() < 0.05 * ana.abs().max(1.0),
                    "splat {i} pos[{a}]: 数值 {num:.4} vs 解析 {ana:.4}"
                );
            }
        }
    }

    #[test]
    fn five_splat_descent_reduces_loss() {
        // 多高斯重叠时，解析梯度与有限差分有偏差（离散的条目集/排序），
        // 但**方向必须能下降**：这里直接检查 20 步位置更新后损失变小。
        let c = cam();
        let (w, h) = (60usize, 60usize);
        let mut pts = Vec::new();
        for i in 0..5 {
            pts.push(Splat {
                pos: [0.02 * i as f32, 0.0, -1.8 + 0.1 * i as f32],
                scale: 0.07,
                color: [0.2 + 0.15 * i as f32, 0.8 - 0.1 * i as f32, 0.3],
                alpha: 0.6,
            });
        }
        let gt = [0.4f32, 0.5, 0.4].repeat(w * h);
        let ids = vec![1u32; w * h];
        let views = vec![(c.clone(), w, h, gt, ids)];
        let before = loss_of(&pts, &views);
        for _ in 0..50 {
            let (_gc, gp, _gs, _ga, cnt) = gradients(&pts, &views, [0.0, 0.0, 0.0]);
            for (i, p) in pts.iter_mut().enumerate() {
                if cnt[i] == 0 {
                    continue;
                }
                let n = f64::from(cnt[i]);
                for (k, pk) in p.pos.iter_mut().enumerate() {
                    *pk -= 5e-3 * (gp[i][k] / n) as f32;
                }
            }
        }
        let after = loss_of(&pts, &views);
        assert!(
            after < before * 0.99,
            "位置更新应降损（方向检查）：{before:.2} → {after:.2}"
        );
    }

    #[test]
    fn psnr_identical_is_infinite() {
        let a = vec![0.5f32; 12];
        assert!(psnr(&a, &a).is_infinite());
        let b = vec![0.0f32; 12];
        let v = psnr(&a, &b);
        assert!(
            (v - 6.0206).abs() < 1e-3,
            "0.5 vs 0 的 PSNR 应为 6.02 dB，得到 {v}"
        );
    }

    #[test]
    fn build_dedups_by_voxel() {
        // 5×5 全前景、深度 2、颜色 (1,0,0)：cell=0.5 m 时点被并成少数几个高斯
        let (w, h) = (5usize, 5usize);
        let view = View {
            ids: vec![1u32; w * h],
            depth: vec![2.0; w * h],
            w,
            h,
            cam: cam(),
        };
        let rgb = vec![1.0f32; w * h * 3];
        let fine = build(
            std::slice::from_ref(&view),
            std::slice::from_ref(&rgb),
            0.01,
            2.0,
        );
        assert!(fine.len() > 10, "细格子应有很多高斯：{}", fine.len());
        let view2 = View {
            ids: vec![1u32; w * h],
            depth: vec![2.0; w * h],
            w,
            h,
            cam: cam(),
        };
        let coarse = build(&[view2], &[rgb], 0.5, 2.0);
        assert!(
            coarse.len() < fine.len(),
            "粗格子应更少：{} vs {}",
            coarse.len(),
            fine.len()
        );
        assert!((coarse[0].color[0] - 1.0).abs() < 1e-6);
    }
}

/// 投影结果（供反传复用）。
#[derive(Clone, Copy)]
pub struct Proj {
    pub u: f64,
    pub v: f64,
    pub d: f64,
    pub sigma: f64,
}

/// 一条「高斯 → 像素」的合成记录：权重 w = alpha · g · pre。
#[derive(Clone, Copy)]
pub struct Entry {
    pub i: u32,
    pub p: u32,
    pub w: f32,
    pub g: f32,
    pub pre: f32,
}

/// 一帧的几何缓存：投影 + 权重记录（位置变化后必须重建）。
/// 一次梯度累加的结果：(颜色, 位置, 尺度, alpha, 每高斯贡献像素数)。
pub type Grads = (Vec<[f64; 3]>, Vec<[f64; 3]>, Vec<f64>, Vec<f64>, Vec<u32>);

pub struct GeomCache {
    pub proj: Vec<Option<Proj>>,
    pub entries: Vec<Entry>,
    pub coverage: Vec<f32>,
}

/// 建缓存（与 `collect` 同一套排序/合成规则，但保留反传所需分量）。
pub fn build_cache(splats: &[Splat], cam: &Camera, w: usize, h: usize) -> GeomCache {
    let fx = cam.k[0][0];
    let mut proj: Vec<Option<Proj>> = vec![None; splats.len()];
    let mut order: Vec<(usize, f64)> = Vec::new();
    for (i, s) in splats.iter().enumerate() {
        let Some((u, v, d)) = cam.project(s.pos) else {
            continue;
        };
        if u < -2.0 || v < -2.0 || u >= w as f64 + 2.0 || v >= h as f64 + 2.0 {
            continue;
        }
        let sigma = (fx * s.scale as f64 / d).max(0.35);
        proj[i] = Some(Proj { u, v, d, sigma });
        order.push((i, d));
    }
    order.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut coverage = vec![0.0f32; w * h];
    let mut entries = Vec::new();
    for (i, _) in order {
        let Some(pr) = proj[i] else { continue };
        let r = (pr.sigma * 2.5).ceil() as i64;
        let (cx, cy) = (pr.u.round() as i64, pr.v.round() as i64);
        for y in (cy - r).max(0)..=(cy + r).min(h as i64 - 1) {
            for x in (cx - r).max(0)..=(cx + r).min(w as i64 - 1) {
                let dx = (x as f64 - pr.u) / pr.sigma;
                let dy = (y as f64 - pr.v) / pr.sigma;
                let g = (-0.5 * (dx * dx + dy * dy)).exp();
                if g < 1e-3 {
                    continue;
                }
                let p = y as usize * w + x as usize;
                let pre = (1.0 - coverage[p]).max(0.0);
                let wgt = splats[i].alpha as f64 * g * pre as f64;
                coverage[p] += wgt as f32;
                entries.push(Entry {
                    i: i as u32,
                    p: p as u32,
                    w: wgt as f32,
                    g: g as f32,
                    pre,
                });
            }
        }
    }
    GeomCache {
        proj,
        entries,
        coverage,
    }
}

/// 用缓存渲染。
pub fn render_cached(
    splats: &[Splat],
    c: &GeomCache,
    w: usize,
    h: usize,
    bg: [f32; 3],
) -> Vec<f32> {
    let mut img = vec![0.0f32; w * h * 3];
    for e in &c.entries {
        let col = splats[e.i as usize].color;
        for k in 0..3 {
            img[e.p as usize * 3 + k] += e.w * col[k];
        }
    }
    for p in 0..w * h {
        let rest = (1.0 - c.coverage[p]).max(0.0);
        for k in 0..3 {
            img[p * 3 + k] += rest * bg[k];
        }
    }
    img
}

/// 拟合开关：哪些量进梯度。
#[derive(Clone, Copy)]
pub struct FitOpts {
    pub iters: usize,
    pub lr_color: f32,
    pub lr_pos: f32,
    pub lr_scale: f32,
    pub lr_alpha: f32,
    /// 位置锚定正则（把高斯拉回初始壳）：0 = 不锚定。
    pub reg: f32,
}

impl Default for FitOpts {
    fn default() -> Self {
        FitOpts {
            iters: 30,
            lr_color: 0.5,
            lr_pos: 0.0015,
            lr_scale: 0.02,
            lr_alpha: 0.05,
            reg: 0.0,
        }
    }
}

/// 一次全参数梯度累加：返回 (颜色, 位置, 尺度, alpha, 每高斯的贡献像素数)。
/// 排序视为局部固定（与 3DGS 同策略），只对合成权重链求导。
pub fn gradients(splats: &[Splat], views: &[FitView], bg: [f32; 3]) -> Grads {
    let mut gc = vec![[0.0f64; 3]; splats.len()];
    let mut gp = vec![[0.0f64; 3]; splats.len()];
    let mut gs = vec![0.0f64; splats.len()];
    let mut ga = vec![0.0f64; splats.len()];
    let mut cnt = vec![0u32; splats.len()];
    let mut acc_pix: HashMap<u32, f64> = HashMap::new();
    for (cam, w, h, gt, ids) in views {
        {
            acc_pix.clear();
            let cache = build_cache(splats, cam, *w, *h);
            let img = render_cached(splats, &cache, *w, *h, bg);
            let r = cam_k(cam);
            for e in cache.entries.iter().rev() {
                let p = e.p as usize;
                if ids[p] == 0 || e.w <= 0.0 {
                    continue;
                }
                let i = e.i as usize;
                let s = &splats[i];
                // dL/dw = Σ_k 2·r_k·(c_k − bg_k)
                let mut dldw = 0.0f64;
                for k in 0..3 {
                    let rk = f64::from(img[p * 3 + k] - gt[p * 3 + k]);
                    gc[i][k] += 2.0 * rk * f64::from(e.w);
                    dldw += 2.0 * rk * f64::from(s.color[k] - bg[k]);
                }
                // 透射率链（逆序）：acc = Σ_{j>i} a_j g_j·direct_j·Π(1−a_l g_l)
                let ag = f64::from(s.alpha) * f64::from(e.g);
                let acc = acc_pix.entry(e.p).or_insert(0.0);
                let dw = dldw - *acc;
                *acc = ag * dldw + (1.0 - ag) * *acc;
                let Some(pr) = cache.proj[i] else { continue };
                let (px, py) = ((p % *w) as f64, (p / *w) as f64);
                let ex = (px - pr.u) / pr.sigma;
                let ey = (py - pr.v) / pr.sigma;
                let g = f64::from(e.g);
                let a = s.alpha as f64;
                let pre = f64::from(e.pre);
                // 权重链：w = a·g·pre
                ga[i] += dw * g * pre;
                let dg = dw * a * pre;
                // g 对 (u, v, σ) 的导数
                let du = dg * g * ex / pr.sigma;
                let dv = dg * g * ey / pr.sigma;
                let dsigma = dg * g * (ex * ex + ey * ey) / pr.sigma;
                gs[i] += dsigma * r.fx / pr.d;
                // σ 也依赖深度；u,v 也依赖深度（见下方雅可比里的 D）
                let dd = -dsigma * pr.sigma / pr.d;
                // u = fx·xb/D, v = fy·(−yb)/D, D = −zb
                let (xb, yb) = {
                    let t = &cam.t_cw;
                    let dx = s.pos[0] as f64 - t[0][3];
                    let dy = s.pos[1] as f64 - t[1][3];
                    let dz = s.pos[2] as f64 - t[2][3];
                    (
                        t[0][0] * dx + t[1][0] * dy + t[2][0] * dz,
                        t[0][1] * dx + t[1][1] * dy + t[2][1] * dz,
                    )
                };
                let ddepth = dd;
                for (a_axis, gpa) in gp[i].iter_mut().enumerate() {
                    let ra0 = cam.t_cw[a_axis][0];
                    let ra1 = cam.t_cw[a_axis][1];
                    let ra2 = cam.t_cw[a_axis][2];
                    let d2 = pr.d * pr.d;
                    let dud = r.fx * (ra0 * pr.d + xb * ra2) / d2;
                    let dvd = r.fy * (-ra1 * pr.d + (-yb) * ra2) / d2;
                    let ddd = -ra2; // D = −zb, ∂D/∂pos_a = −R[a][2]
                    *gpa += du * dud + dv * dvd + ddepth * ddd;
                }
                cnt[i] += 1;
            }
        }
    }
    (gc, gp, gs, ga, cnt)
}

/// 用实心表面的「未覆盖格」补高斯（补背面/底面）。
/// 颜色取最近的已观测高斯；找不到就取全局平均。返回补的个数。
pub fn fill_unobserved(
    splats: &mut Vec<Splat>,
    solid: &crate::mesh::Grid,
    surface: &[bool],
    scale: f32,
    alpha: f32,
) -> usize {
    use std::collections::HashSet;
    let cell = solid.cell;
    let key = |p: [f32; 3]| {
        (
            ((p[0] - solid.origin[0]) / cell).floor() as i32,
            ((p[1] - solid.origin[1]) / cell).floor() as i32,
            ((p[2] - solid.origin[2]) / cell).floor() as i32,
        )
    };
    let mut occupied: HashSet<(i32, i32, i32)> = HashSet::new();
    let mut colmap: HashMap<(i32, i32, i32), [f32; 3]> = HashMap::new();
    let mut sum = [0.0f64; 3];
    for s in splats.iter() {
        let k = key(s.pos);
        occupied.insert(k);
        colmap.insert(k, s.color);
        for (acc, c) in sum.iter_mut().zip(s.color.iter()) {
            *acc += *c as f64;
        }
    }
    let mean = if splats.is_empty() {
        [0.5; 3]
    } else {
        [0, 1, 2].map(|a| (sum[a] / splats.len() as f64) as f32)
    };
    let mut added = 0usize;
    for z in 0..solid.nz {
        for y in 0..solid.ny {
            for x in 0..solid.nx {
                let i = x + solid.nx * (y + solid.ny * z);
                if !surface[i] {
                    continue;
                }
                let k = (x as i32, y as i32, z as i32);
                if occupied.contains(&k) {
                    continue;
                }
                let mut color = None;
                'search: for r in 1..=4i32 {
                    for dz in -r..=r {
                        for dy in -r..=r {
                            for dx in -r..=r {
                                if dx.abs() + dy.abs() + dz.abs() != r {
                                    continue;
                                }
                                if let Some(c) =
                                    colmap.get(&(x as i32 + dx, y as i32 + dy, z as i32 + dz))
                                {
                                    color = Some(*c);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
                splats.push(Splat {
                    pos: [
                        solid.origin[0] + (x as f32 + 0.5) * cell,
                        solid.origin[1] + (y as f32 + 0.5) * cell,
                        solid.origin[2] + (z as f32 + 0.5) * cell,
                    ],
                    scale,
                    color: color.unwrap_or(mean),
                    alpha,
                });
                added += 1;
            }
        }
    }
    added
}

/// 自适应增删（3DGS 的 densify/prune 简化版）。
/// 每轮先渲染拟合视角：前景里残差 > `err` 的像素，在观测表面（深度反投影）补一个高斯
/// （颜色取 GT、半径 = `scale`）；然后删掉 alpha < `prune` 的高斯。
/// 返回 (补的个数, 删的个数)。
pub fn densify(
    splats: &mut Vec<Splat>,
    views: &[FitView],
    depths: &[Vec<f32>],
    scale: f32,
    err: f32,
    prune: f32,
    max_add: usize,
) -> (usize, usize) {
    let mut add: HashMap<(i64, i64, i64), Splat> = HashMap::new();
    let inv = 1.0 / (scale.max(1e-5) * 2.0);
    for (vi, (cam, w, h, gt, ids)) in views.iter().enumerate() {
        let dep = &depths[vi];
        let cache = build_cache(splats, cam, *w, *h);
        let img = render_cached(splats, &cache, *w, *h, [0.0; 3]);
        for p in 0..*w * *h {
            if ids[p] == 0 || !(dep[p].is_finite() && dep[p] > 0.0) {
                continue;
            }
            let mut e = 0.0f32;
            for k in 0..3 {
                e = e.max((img[p * 3 + k] - gt[p * 3 + k]).abs());
            }
            if e <= err {
                continue;
            }
            let pos = cam.unproject((p % *w) as f64, (p / *w) as f64, dep[p]);
            let key = (
                (pos[0] / (scale * 2.0)).floor() as i64,
                (pos[1] / (scale * 2.0)).floor() as i64,
                (pos[2] / (scale * 2.0)).floor() as i64,
            );
            add.entry(key).or_insert(Splat {
                pos,
                scale,
                color: [gt[p * 3], gt[p * 3 + 1], gt[p * 3 + 2]],
                alpha: 0.6,
            });
            let _ = inv;
        }
    }
    // 删：alpha 太低
    let before = splats.len();
    splats.retain(|s| s.alpha >= prune);
    let pruned = before - splats.len();
    let mut added = 0;
    for s in add.into_values().take(max_add) {
        splats.push(s);
        added += 1;
    }
    (added, pruned)
}

/// 全参数拟合（颜色 + 位置 + 尺度 + alpha）。
pub fn fit_all(splats: &mut [Splat], views: &[FitView], opt: FitOpts, bg: [f32; 3]) {
    if splats.is_empty() || views.is_empty() {
        return;
    }
    let anchors: Vec<[f32; 3]> = splats.iter().map(|s| s.pos).collect();
    for _ in 0..opt.iters {
        let (gc, mut gp, gs, ga, cnt) = gradients(splats, views, bg);
        if opt.reg > 0.0 {
            for (i, s) in splats.iter().enumerate() {
                for k in 0..3 {
                    gp[i][k] += f64::from(opt.reg) * f64::from(s.pos[k] - anchors[i][k]);
                }
            }
        }
        for (i, s) in splats.iter_mut().enumerate() {
            if cnt[i] == 0 {
                continue;
            }
            let n = f64::from(cnt[i]);
            for k in 0..3 {
                s.color[k] = (s.color[k] - opt.lr_color * (gc[i][k] / n) as f32).clamp(0.0, 1.0);
                s.pos[k] -= opt.lr_pos * (gp[i][k] / n) as f32;
            }
            s.scale = (s.scale - opt.lr_scale * (gs[i] / n) as f32).max(1e-4);
            s.alpha = (s.alpha - opt.lr_alpha * (ga[i] / n) as f32).clamp(1e-3, 0.999);
        }
    }
}

/// 取相机内参里渲染要用的量（fx, fy）。
struct CamK {
    fx: f64,
    fy: f64,
}

fn cam_k(cam: &Camera) -> CamK {
    CamK {
        fx: cam.k[0][0],
        fy: cam.k[1][1],
    }
}
