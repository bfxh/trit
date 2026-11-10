use crate::mesh::Mesh;

/// 一个 chart：属于它的三角形下标 + 在图集里的矩形 `[x, y, w, h]`（像素）。
#[derive(Clone, Debug)]
pub struct Chart {
    pub tris: Vec<usize>,
    pub rect: [usize; 4],
}

/// 烘好的图集。
pub struct Atlas {
    /// 交错 RGB，`w×h`。
    pub img: Vec<f32>,
    /// 每类被风格改写的纹素数（无风格时全 0）——「按类别分开」的精确判据。
    pub styled_texels: Vec<usize>,
    pub w: usize,
    pub h: usize,
    pub charts: Vec<Chart>,
    /// 每个三角形三个角的 `(u,v)`，与 `Mesh::tris` 一一对应。
    pub uv: Vec<[[f32; 2]; 3]>,
}

/// 面法线的主轴（0=x, 1=y, 2=z）。
pub(crate) fn sample(tex: &[f32], tw: usize, th: usize, u: f32, v: f32) -> [f32; 3] {
    let x = ((u.rem_euclid(1.0) * tw as f32) as usize).min(tw.saturating_sub(1));
    let y = (((1.0 - v.rem_euclid(1.0)) * th as f32) as usize).min(th.saturating_sub(1));
    let i = (y * tw + x) * 3;
    [tex[i], tex[i + 1], tex[i + 2]]
}

pub(crate) fn find(p: &mut [usize], x: usize) -> usize {
    if p[x] != x {
        let r = find(p, p[x]);
        p[x] = r;
    }
    p[x]
}

/// 三角形法线（未归一化）与面积。
pub(crate) fn tri_normal(m: &Mesh, t: &[u32; 3]) -> [f32; 3] {
    let a = m.verts[t[0] as usize];
    let b = m.verts[t[1] as usize];
    let c = m.verts[t[2] as usize];
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ]
}

pub(crate) fn norm(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

pub(crate) fn tri_area(m: &Mesh, t: &[u32; 3]) -> f32 {
    0.5 * norm(tri_normal(m, t))
}

/// CGNR（最小二乘的共轭梯度正规方程）：解 `min ||A x − b||`，纯浮点、确定性。
/// `rows` 为稀疏行（列号, 系数）；`x0` 给初值（用平面投影当初值收敛很快）。
fn cgnr(rows: &[(Vec<(usize, f32)>, f32)], ncol: usize, x0: Vec<f32>, iters: usize) -> Vec<f32> {
    let n = rows.len();
    let ax = |x: &[f32]| -> Vec<f32> {
        let mut y = vec![0.0f32; n];
        for (i, (r, _)) in rows.iter().enumerate() {
            let mut s = 0.0f32;
            for &(c, v) in r {
                s += v * x[c];
            }
            y[i] = s;
        }
        y
    };
    let atv = |y: &[f32]| -> Vec<f32> {
        let mut o = vec![0.0f32; ncol];
        for (i, (r, _)) in rows.iter().enumerate() {
            if y[i] == 0.0 {
                continue;
            }
            for &(c, v) in r {
                o[c] += v * y[i];
            }
        }
        o
    };
    let b: Vec<f32> = rows.iter().map(|(_, b)| *b).collect();
    let mut x = x0;
    let mut r = b.clone();
    let axv = ax(&x);
    for i in 0..n {
        r[i] -= axv[i];
    }
    let mut p = atv(&r);
    let mut gamma: f32 = p.iter().map(|v| v * v).sum();
    let bb: f32 = b.iter().map(|v| v * v).sum::<f32>().max(1e-12);
    for _ in 0..iters {
        let q = ax(&p);
        let qq: f32 = q.iter().map(|v| v * v).sum();
        if qq < 1e-20 || gamma < 1e-12 * bb {
            break;
        }
        let alpha = gamma / qq;
        for i in 0..ncol {
            x[i] += alpha * p[i];
        }
        for i in 0..n {
            r[i] -= alpha * q[i];
        }
        let s2 = atv(&r);
        let g2: f32 = s2.iter().map(|v| v * v).sum();
        let beta = g2 / gamma.max(1e-20);
        for i in 0..ncol {
            p[i] = s2[i] + beta * p[i];
        }
        gamma = g2;
    }
    x
}

/// 单个 chart 的 LSCM：把 `pts3d`（该 chart 的局部顶点）映到 2D，保角优先。
///
/// 做法：用面积加权平均法线建切平面得到「局部 2D 坐标」（同时给 CR 系统当参照系），
/// 每个三角形写两条 Cauchy–Riemann 约束（按 √面积 加权），钉住相距最远的两个点，
/// 用 CGNR 解最小二乘。返回每个顶点在**世界尺度**下的 (u, v)。
pub(crate) fn lscm_chart(pts3d: &[[f32; 3]], tris: &[[u32; 3]]) -> Vec<[f32; 2]> {
    let nv = pts3d.len();
    if nv < 3 || tris.is_empty() {
        return pts3d.iter().map(|p| [p[0], p[1]]).collect();
    }
    // 切平面（面积加权平均法线）
    let mut acc = [0.0f32; 3];
    for t in tris {
        let (a, b, c) = (
            pts3d[t[0] as usize],
            pts3d[t[1] as usize],
            pts3d[t[2] as usize],
        );
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        for d in 0..3 {
            acc[d] += n[d];
        }
    }
    let l = norm(acc).max(1e-12);
    let nv3 = [acc[0] / l, acc[1] / l, acc[2] / l];
    let up = if nv3[1].abs() < 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let mut ux = [
        up[1] * nv3[2] - up[2] * nv3[1],
        up[2] * nv3[0] - up[0] * nv3[2],
        up[0] * nv3[1] - up[1] * nv3[0],
    ];
    let lu = norm(ux).max(1e-12);
    ux = [ux[0] / lu, ux[1] / lu, ux[2] / lu];
    let vx = [
        nv3[1] * ux[2] - nv3[2] * ux[1],
        nv3[2] * ux[0] - nv3[0] * ux[2],
        nv3[0] * ux[1] - nv3[1] * ux[0],
    ];
    let frame: Vec<[f32; 2]> = pts3d
        .iter()
        .map(|p| {
            [
                p[0] * ux[0] + p[1] * ux[1] + p[2] * ux[2],
                p[0] * vx[0] + p[1] * vx[1] + p[2] * vx[2],
            ]
        })
        .collect();
    // 未知量：每个顶点的 (u, v)
    let idx = |v: usize| 2 * v;
    let mut rows: Vec<(Vec<(usize, f32)>, f32)> = Vec::with_capacity(tris.len() * 2 + 4);
    let mut area_sum = 0.0f32;
    for t in tris {
        let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
        // **每个三角形自己的等距局部坐标**（不是全 chart 统一切平面）：
        // 用统一切平面的话，恒等映射天然满足 CR，LSCM 会退回原投影（实测 spread 完全不变）。
        let (a, b, c) = (pts3d[i0], pts3d[i1], pts3d[i2]);
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let l1 = norm(e1);
        if l1 < 1e-12 {
            continue;
        }
        let ax = [e1[0] / l1, e1[1] / l1, e1[2] / l1];
        let d = e2[0] * ax[0] + e2[1] * ax[1] + e2[2] * ax[2];
        let h2 = norm(e2) * norm(e2) - d * d;
        if h2 < 1e-16 {
            continue;
        }
        let p0 = [0.0f32, 0.0f32];
        let p1 = [l1, 0.0f32];
        let p2 = [d, h2.sqrt()];
        let area2 = l1 * p2[1];
        let w = (0.5 * area2).sqrt();
        let inv = 1.0 / area2;
        // 线性基函数梯度（未除以 2A 时是 (y_j−y_k, x_k−x_j)，再乘 1/area2）
        let g = [
            [(p1[1] - p2[1]) * inv, (p2[0] - p1[0]) * inv],
            [(p2[1] - p0[1]) * inv, (p0[0] - p2[0]) * inv],
            [(p0[1] - p1[1]) * inv, (p1[0] - p0[0]) * inv],
        ];
        let ids = [i0, i1, i2];
        // CR: ∂u/∂x − ∂v/∂y = 0 ；∂u/∂y + ∂v/∂x = 0
        let mut r1: Vec<(usize, f32)> = Vec::with_capacity(6);
        let mut r2: Vec<(usize, f32)> = Vec::with_capacity(6);
        for k in 0..3 {
            let v = ids[k];
            r1.push((idx(v), g[k][0] * w));
            r1.push((idx(v) + 1, -g[k][1] * w));
            r2.push((idx(v), g[k][1] * w));
            r2.push((idx(v) + 1, g[k][0] * w));
        }
        rows.push((r1, 0.0));
        rows.push((r2, 0.0));
        area_sum += 0.5 * area2.abs();
    }
    // 钉两点：切平面里相距最远的两个顶点（用粗采样找，避免 O(n²)）
    let step = (nv / 256).max(1);
    let mut best = (0usize, 0usize, -1.0f32);
    let mut i = 0;
    while i < nv {
        let mut j = i + step;
        while j < nv {
            let d = (frame[i][0] - frame[j][0]).powi(2) + (frame[i][1] - frame[j][1]).powi(2);
            if d > best.2 {
                best = (i, j, d);
            }
            j += step * 4;
        }
        i += step;
    }
    let pin_w = 1e3 * area_sum.sqrt().max(1e-6);
    for (v, frame_uv) in [(best.0, frame[best.0]), (best.1, frame[best.1])] {
        rows.push((
            vec![(idx(v), pin_w), (idx(v) + 1, pin_w * 0.0 + pin_w * 0.0)],
            pin_w * frame_uv[0],
        ));
        // 上面那行只钉了 u；v 单独一行（分开写，别让 0 系数进矩阵）
        rows.pop();
        rows.push((vec![(idx(v), pin_w)], pin_w * frame_uv[0]));
        rows.push((vec![(idx(v) + 1, pin_w)], pin_w * frame_uv[1]));
    }
    let mut x0 = vec![0.0f32; 2 * nv];
    for v in 0..nv {
        x0[2 * v] = frame[v][0];
        x0[2 * v + 1] = frame[v][1];
    }
    let x = cgnr(&rows, 2 * nv, x0, 800);
    (0..nv).map(|v| [x[2 * v], x[2 * v + 1]]).collect()
}

/// 逐三角形类别 + 每类点式风格（模糊这类空间算子不参与）。
pub struct PerClass<'a> {
    pub class: &'a [u8],
    pub styles: &'a [crate::tex::Style],
}

pub(crate) fn style_texel(
    c: [f32; 3],
    s: &crate::tex::Style,
    mean: [f32; 3],
    lo: f32,
    hi: f32,
) -> [f32; 3] {
    let mut p = c;
    if (s.contrast - 1.0).abs() > 1e-6 {
        for k in 0..3 {
            p[k] = ((p[k] - mean[k]) * s.contrast + mean[k]).clamp(0.0, 1.0);
        }
    }
    if s.warmth.abs() > 1e-6 {
        p[0] = (p[0] * (1.0 + s.warmth)).clamp(0.0, 1.0);
        p[2] = (p[2] * (1.0 - s.warmth)).clamp(0.0, 1.0);
    }
    if (s.sat - 1.0).abs() > 1e-6 {
        let y = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        for v in p.iter_mut() {
            *v = (y + (*v - y) * s.sat).clamp(0.0, 1.0);
        }
    }
    if s.levels >= 2 && hi - lo > 1e-6 {
        let n = s.levels as f32 - 1.0;
        for v in p.iter_mut() {
            let t = ((*v - lo) / (hi - lo) * n).round() / n;
            *v = lo + t * (hi - lo);
        }
    }
    p
}
