use std::collections::HashMap;

use crate::geo::Camera;

use super::core::*;

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
