//! 三角形光栅化：精确覆盖率（top-left 规则）+ 透视校正属性 + 解析屏幕导数 + z-buffer。
//!
//! 覆盖判定用 f64 计算边函数。顶点坐标本身就是 f32，转 f64 无损；f64 下两个乘积
//! （各 ≤48 位有效位）相加也**精确**，所以共享边两侧算出的边函数严格互为相反数，
//! 配合 top-left 规则可证明「每个像素恰好着色一次」——这一条有测试钉住。
//!
//! 导数用解析式而不是 2×2 quad 差分：`u = Nu/W`，`Nu`、`W` 都是屏幕空间仿射函数，
//! 于是 `∂u/∂x = (∂Nu/∂x − u·∂W/∂x)/W`。比 GPU 的有限差分更准，也不需要 helper
//! 调用；代价是每像素多几次乘加。
//!
//! **不做裁剪**：`w ≤ 0` 的三角形整块跳过，近/远平面外的像素只按 z-buffer 处理。

use super::mesh::{Mesh, Vertex};

/// 屏幕空间导数：UV 对屏幕 x/y 的偏导。
///
/// 与 `trit_texture::sample::lod_from_derivatives` 同口径（纹素/像素）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Derivative {
    /// `∂u/∂x`
    pub du_x: f32,
    /// `∂v/∂x`
    pub dv_x: f32,
    /// `∂u/∂y`
    pub du_y: f32,
    /// `∂v/∂y`
    pub dv_y: f32,
}

/// 一个通过覆盖与深度测试的像素。
#[derive(Clone, Copy, Debug)]
pub struct Fragment {
    /// 像素列。
    pub x: usize,
    /// 像素行。
    pub y: usize,
    /// 透视校正后的纹理坐标。
    pub uv: [f32; 2],
    /// 屏幕空间导数（可直接喂给采样器的 LOD/各向异性）。
    pub derivative: Derivative,
    /// 该像素的深度（NDC z，`[0, 1]`，越小越近）。
    pub depth: f32,
}

/// 着色结果。
#[derive(Clone, Copy, Debug)]
pub struct Shaded {
    /// RGB。
    pub color: [u8; 3],
    /// 该像素实际用的 LOD（`None` = 这个口径没有 LOD，不计入统计）。
    pub lod: Option<f32>,
}

impl Shaded {
    /// 只给颜色（不参与 LOD 统计）。
    pub fn rgb(color: [u8; 3]) -> Self {
        Self { color, lod: None }
    }
}

/// 渲染目标：RGB 颜色 + 深度。
pub struct Target {
    /// 宽（像素）。
    pub width: usize,
    /// 高（像素）。
    pub height: usize,
    /// 颜色缓冲（行优先，RGB）。
    pub color: Vec<[u8; 3]>,
    /// 深度缓冲（NDC z，初值 `+∞`）。
    pub depth: Vec<f32>,
}

impl Target {
    /// 新建目标并填背景色（深度清成 `+∞`）。
    pub fn new(width: usize, height: usize, background: [u8; 3]) -> Self {
        Self {
            width,
            height,
            color: vec![background; width * height],
            depth: vec![f32::INFINITY; width * height],
        }
    }
}

/// 光栅化统计。
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// 处理的三角形数（含被跳过的）。
    pub triangles: usize,
    /// 通过覆盖测试的像素数（含后来被 z 挡下的）。
    pub covered: usize,
    /// 通过深度测试、真正着色的像素数。
    pub shaded: usize,
    /// 退化三角形（面积 0 / 非有限坐标）。
    pub skipped_degenerate: usize,
    /// `w ≤ 0` 的三角形（需要裁剪，本实现不支持）。
    pub skipped_behind: usize,
    /// 着色像素用过的最小 LOD。
    pub lod_min: f32,
    /// 着色像素用过的最大 LOD。
    pub lod_max: f32,
    /// 着色像素的 LOD 均值。
    pub lod_mean: f32,
}

/// 三角形绕向归一化后的顶点（屏幕空间 + 透视校正所需属性）。
struct Setup {
    x: [f64; 3],
    y: [f64; 3],
    z: [f64; 3],
    inv_w: [f64; 3],
    u_over_w: [f64; 3],
    v_over_w: [f64; 3],
    area: f64,
    top_left: [bool; 3],
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
}

#[inline]
fn edge(ax: f64, ay: f64, bx: f64, by: f64, px: f64, py: f64) -> f64 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

/// top-left 规则（y 向下的屏幕空间，面积为正）：`dy > 0`，或水平且 `dx < 0`。
#[inline]
fn is_top_left(ax: f64, ay: f64, bx: f64, by: f64) -> bool {
    let dy = by - ay;
    let dx = bx - ax;
    dy > 0.0 || (dy == 0.0 && dx < 0.0)
}

/// 屏幕空间仿射量 `a` 的梯度 `(∂/∂x, ∂/∂y)`（`a` 的三个顶点值）。
#[inline]
fn plane(a: [f64; 3], s: &Setup) -> (f64, f64) {
    let dx = a[0] * (s.y[1] - s.y[2]) + a[1] * (s.y[2] - s.y[0]) + a[2] * (s.y[0] - s.y[1]);
    let dy = a[0] * (s.x[2] - s.x[1]) + a[1] * (s.x[0] - s.x[2]) + a[2] * (s.x[1] - s.x[0]);
    (dx / s.area, dy / s.area)
}

fn setup(tri: &[Vertex; 3], width: usize, height: usize) -> Option<Setup> {
    let mut verts = *tri;
    let mut w = [0f64; 3];
    for (slot, v) in verts.iter().enumerate() {
        if !v.clip.iter().all(|c| c.is_finite()) {
            return None;
        }
        w[slot] = v.clip[3] as f64;
        if w[slot] <= 0.0 {
            return None;
        }
    }
    let (fw, fh) = (width as f64, height as f64);
    let mut x = [0f64; 3];
    let mut y = [0f64; 3];
    let mut z = [0f64; 3];
    for i in 0..3 {
        let ndc_x = verts[i].clip[0] as f64 / w[i];
        let ndc_y = verts[i].clip[1] as f64 / w[i];
        x[i] = (ndc_x * 0.5 + 0.5) * fw;
        y[i] = (0.5 - ndc_y * 0.5) * fh;
        z[i] = verts[i].clip[2] as f64 / w[i];
    }
    let mut area = edge(x[0], y[0], x[1], y[1], x[2], y[2]);
    if area == 0.0 || !area.is_finite() {
        return None;
    }
    if area < 0.0 {
        // 归一化成面积为正：交换 1/2，后面所有逐顶点属性一起换
        verts.swap(1, 2);
        w.swap(1, 2);
        x.swap(1, 2);
        y.swap(1, 2);
        z.swap(1, 2);
        area = -area;
    }
    let inv_w = [1.0 / w[0], 1.0 / w[1], 1.0 / w[2]];
    let u_over_w = [
        verts[0].uv[0] as f64 * inv_w[0],
        verts[1].uv[0] as f64 * inv_w[1],
        verts[2].uv[0] as f64 * inv_w[2],
    ];
    let v_over_w = [
        verts[0].uv[1] as f64 * inv_w[0],
        verts[1].uv[1] as f64 * inv_w[1],
        verts[2].uv[1] as f64 * inv_w[2],
    ];
    let top_left = [
        is_top_left(x[1], y[1], x[2], y[2]),
        is_top_left(x[2], y[2], x[0], y[0]),
        is_top_left(x[0], y[0], x[1], y[1]),
    ];
    let min_x = x.iter().cloned().fold(f64::INFINITY, f64::min).max(0.0) as usize;
    let min_y = y.iter().cloned().fold(f64::INFINITY, f64::min).max(0.0) as usize;
    let max_x = (x.iter().cloned().fold(f64::NEG_INFINITY, f64::max).ceil() as usize).min(width);
    let max_y = (y.iter().cloned().fold(f64::NEG_INFINITY, f64::max).ceil() as usize).min(height);
    if min_x >= max_x || min_y >= max_y {
        return None;
    }
    Some(Setup {
        x,
        y,
        z,
        inv_w,
        u_over_w,
        v_over_w,
        area,
        top_left,
        min_x,
        max_x,
        min_y,
        max_y,
    })
}

/// 光栅化整个网格：覆盖 → 深度测试 → 调 `shade` 写颜色。
///
/// 两条绕向都接受（内部按面积符号归一化），不做背面剔除。
pub fn rasterize<F>(mesh: &Mesh, target: &mut Target, mut shade: F) -> Stats
where
    F: FnMut(&Fragment) -> Shaded,
{
    let mut stats = Stats::default();
    if target.width == 0 || target.height == 0 {
        return stats;
    }
    let (mut lod_sum, mut lod_count) = (0f64, 0usize);
    let mut lod_min = f32::INFINITY;
    let mut lod_max = f32::NEG_INFINITY;
    for tri in &mesh.indices {
        stats.triangles += 1;
        let (Some(a), Some(b), Some(c)) = (
            mesh.vertices.get(tri[0] as usize),
            mesh.vertices.get(tri[1] as usize),
            mesh.vertices.get(tri[2] as usize),
        ) else {
            stats.skipped_degenerate += 1;
            continue;
        };
        let verts = [*a, *b, *c];
        if verts
            .iter()
            .any(|v| v.clip[3] <= 0.0 || !v.clip[3].is_finite())
        {
            stats.skipped_behind += 1;
            continue;
        }
        let Some(s) = setup(&verts, target.width, target.height) else {
            stats.skipped_degenerate += 1;
            continue;
        };
        let (dw_x, dw_y) = plane(s.inv_w, &s);
        let (dnu_x, dnu_y) = plane(s.u_over_w, &s);
        let (dnv_x, dnv_y) = plane(s.v_over_w, &s);
        for py in s.min_y..s.max_y {
            let pyf = py as f64 + 0.5;
            for px in s.min_x..s.max_x {
                let pxf = px as f64 + 0.5;
                let e0 = edge(s.x[1], s.y[1], s.x[2], s.y[2], pxf, pyf);
                let e1 = edge(s.x[2], s.y[2], s.x[0], s.y[0], pxf, pyf);
                let e2 = edge(s.x[0], s.y[0], s.x[1], s.y[1], pxf, pyf);
                let lids = [e0, e1, e2];
                let inside = lids
                    .iter()
                    .enumerate()
                    .all(|(i, &e)| e > 0.0 || (e == 0.0 && s.top_left[i]));
                if !inside {
                    continue;
                }
                stats.covered += 1;
                let l = [e0 / s.area, e1 / s.area, e2 / s.area];
                let depth = l[0] * s.z[0] + l[1] * s.z[1] + l[2] * s.z[2];
                let idx = py * target.width + px;
                if depth >= target.depth[idx] as f64 {
                    continue;
                }
                let inv_w = l[0] * s.inv_w[0] + l[1] * s.inv_w[1] + l[2] * s.inv_w[2];
                let nu = l[0] * s.u_over_w[0] + l[1] * s.u_over_w[1] + l[2] * s.u_over_w[2];
                let nv = l[0] * s.v_over_w[0] + l[1] * s.v_over_w[1] + l[2] * s.v_over_w[2];
                let u = nu / inv_w;
                let v = nv / inv_w;
                let derivative = Derivative {
                    du_x: ((dnu_x - u * dw_x) / inv_w) as f32,
                    dv_x: ((dnv_x - v * dw_x) / inv_w) as f32,
                    du_y: ((dnu_y - u * dw_y) / inv_w) as f32,
                    dv_y: ((dnv_y - v * dw_y) / inv_w) as f32,
                };
                let fragment = Fragment {
                    x: px,
                    y: py,
                    uv: [u as f32, v as f32],
                    derivative,
                    depth: depth as f32,
                };
                let shaded = shade(&fragment);
                target.color[idx] = shaded.color;
                target.depth[idx] = depth as f32;
                stats.shaded += 1;
                if let Some(lod) = shaded.lod {
                    lod_sum += lod as f64;
                    lod_count += 1;
                    lod_min = lod_min.min(lod);
                    lod_max = lod_max.max(lod);
                }
            }
        }
    }
    if lod_count > 0 {
        stats.lod_min = lod_min;
        stats.lod_max = lod_max;
        stats.lod_mean = (lod_sum / lod_count as f64) as f32;
    }
    stats
}
