//! 图集烘焙：按「主轴 + 连通分量」切 chart，线性投影后用货架法打包，
//! 把源贴图烘进一张单图集，并给出每个三角形三个角的 UV。
//!
//! 与 [`crate::uv::box_uvs`] 的区别：box_uvs 让 UV 直接超出 [0,1] 靠 wrap 平铺；
//! 这里每个 chart 独占一块图集区域，贴图只被采样一次，适合把 quilt 结果固化成单张资产。
//! 局限：chart 边界仍有接缝（盒投影的固有性质）；重叠区域（如背面被投影到同一 chart）
//! 会共享同一块 UV，不保证 bijective。

use crate::mesh::Mesh;
use std::collections::HashMap;

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
fn sample(tex: &[f32], tw: usize, th: usize, u: f32, v: f32) -> [f32; 3] {
    let x = ((u.rem_euclid(1.0) * tw as f32) as usize).min(tw.saturating_sub(1));
    let y = (((1.0 - v.rem_euclid(1.0)) * th as f32) as usize).min(th.saturating_sub(1));
    let i = (y * tw + x) * 3;
    [tex[i], tex[i + 1], tex[i + 2]]
}

fn find(p: &mut [usize], x: usize) -> usize {
    if p[x] != x {
        let r = find(p, p[x]);
        p[x] = r;
    }
    p[x]
}

/// 三角形法线（未归一化）与面积。
fn tri_normal(m: &Mesh, t: &[u32; 3]) -> [f32; 3] {
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

fn norm(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn tri_area(m: &Mesh, t: &[u32; 3]) -> f32 {
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
fn lscm_chart(pts3d: &[[f32; 3]], tris: &[[u32; 3]]) -> Vec<[f32; 2]> {
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

fn style_texel(c: [f32; 3], s: &crate::tex::Style, mean: [f32; 3], lo: f32, hi: f32) -> [f32; 3] {
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

/// 切 chart + 打包 + 烘焙。
///
/// chart 用**法线相似度**聚类（相邻面法线夹角 <= `normal_deg` 才合并）。
/// `use_lscm=false` 时每 chart 用面积加权平均法线做**正交投影**（快，曲面会拉伸）；
/// `use_lscm=true` 时每 chart 解 **LSCM**（保角，密度更均匀，代价是慢几十倍）。
///
/// `texels_per_unit <= 0` 时按总面积自动定密度（目标填充 60%）；放不下会减半重排。
#[allow(clippy::too_many_arguments)]
pub fn bake_atlas(
    m: &Mesh,
    tex: &[f32],
    tw: usize,
    th: usize,
    atlas_px: usize,
    texels_per_unit: f32,
    normal_deg: f32,
    use_lscm: bool,
    per_class: Option<&PerClass>,
) -> Result<Atlas, String> {
    if m.tris.is_empty() {
        return Err("空网格".into());
    }
    if tex.len() != tw * th * 3 {
        return Err(format!("贴图像素数 {} != {tw}x{th}x3", tex.len()));
    }
    let n = m.tris.len();
    let nrm: Vec<[f32; 3]> = m
        .tris
        .iter()
        .map(|t| {
            let v = tri_normal(m, t);
            let l = norm(v).max(1e-12);
            [v[0] / l, v[1] / l, v[2] / l]
        })
        .collect();
    let areas: Vec<f32> = m.tris.iter().map(|t| tri_area(m, t)).collect();
    let mut e2f: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (fi, t) in m.tris.iter().enumerate() {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = if a < b { (a, b) } else { (b, a) };
            e2f.entry(e).or_default().push(fi);
        }
    }
    let cos_thr = normal_deg.to_radians().cos();
    let mut parent: Vec<usize> = (0..n).collect();
    for fs in e2f.values() {
        for i in 0..fs.len() {
            for j in i + 1..fs.len() {
                let (a, b) = (fs[i], fs[j]);
                let d = nrm[a][0] * nrm[b][0] + nrm[a][1] * nrm[b][1] + nrm[a][2] * nrm[b][2];
                if d >= cos_thr {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    if ra != rb {
                        parent[ra] = rb;
                    }
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for fi in 0..n {
        let r = find(&mut parent, fi);
        groups.entry(r).or_default().push(fi);
    }
    let mut group_list: Vec<Vec<usize>> = groups.into_values().collect();
    group_list.sort_by_key(|g| g[0]);

    // 每个 chart 的三条角点 2D 坐标（世界尺度）
    let mut coords: Vec<Vec<[[f32; 2]; 3]>> = Vec::with_capacity(group_list.len());
    for g in &group_list {
        // 切平面（LSCM 也用它当局部参照系与初值）
        let mut acc = [0.0f32; 3];
        for &fi in g {
            for c in 0..3 {
                acc[c] += nrm[fi][c] * areas[fi].max(1e-12);
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
        let proj_p = |p: [f32; 3]| -> [f32; 2] {
            [
                p[0] * ux[0] + p[1] * ux[1] + p[2] * ux[2],
                p[0] * vx[0] + p[1] * vx[1] + p[2] * vx[2],
            ]
        };
        let mut per_tri: Vec<[[f32; 2]; 3]> = Vec::with_capacity(g.len());
        if use_lscm {
            // 局部顶点编号 → LSCM 结果
            let mut local: HashMap<u32, usize> = HashMap::new();
            let mut pts: Vec<[f32; 3]> = Vec::new();
            let mut ltris: Vec<[u32; 3]> = Vec::with_capacity(g.len());
            for &fi in g {
                let mut t = [0u32; 3];
                for (k, slot) in t.iter_mut().enumerate() {
                    let vid = m.tris[fi][k];
                    let id = *local.entry(vid).or_insert_with(|| {
                        pts.push(m.verts[vid as usize]);
                        pts.len() - 1
                    });
                    *slot = id as u32;
                }
                ltris.push(t);
            }
            let mut puv = lscm_chart(&pts, &ltris);
            // LSCM 只保角、不保尺度，且逐 chart 解的绝对尺度会漂 ⇒ 按面积归一到世界尺度
            {
                let mut a3 = 0.0f32;
                let mut a2 = 0.0f32;
                for (li, &fi) in g.iter().enumerate() {
                    a3 += areas[fi];
                    let t = ltris[li];
                    let (p0, p1, p2) = (puv[t[0] as usize], puv[t[1] as usize], puv[t[2] as usize]);
                    a2 += 0.5
                        * ((p1[0] - p0[0]) * (p2[1] - p0[1]) + (p2[0] - p0[0]) * (p0[1] - p1[1]))
                            .abs();
                }
                if a2 > 1e-12 && a3 > 1e-12 {
                    let k = (a3 / a2).sqrt();
                    for p in puv.iter_mut() {
                        p[0] *= k;
                        p[1] *= k;
                    }
                }
            }
            let mut lscm_tri: Vec<[[f32; 2]; 3]> = Vec::with_capacity(g.len());
            for &fi in g {
                let mut tri = [[0.0f32; 2]; 3];
                for k in 0..3 {
                    tri[k] = puv[local[&m.tris[fi][k]]];
                }
                lscm_tri.push(tri);
            }
            let mut planar_tri: Vec<[[f32; 2]; 3]> = Vec::with_capacity(g.len());
            for &fi in g {
                planar_tri.push([
                    proj_p(m.verts[m.tris[fi][0] as usize]),
                    proj_p(m.verts[m.tris[fi][1] as usize]),
                    proj_p(m.verts[m.tris[fi][2] as usize]),
                ]);
            }
            // 逐 chart 择优：CG 在病态 chart 上会塌缩，LSCM 不比平面好就退回平面（保证不劣化）
            let spread = |tris2: &[Vec<[[f32; 2]; 3]>]| -> f32 {
                let mut d: Vec<f32> = Vec::new();
                for (li, &fi) in g.iter().enumerate() {
                    let wa = areas[fi];
                    if wa < 1e-12 {
                        continue;
                    }
                    let p = &tris2[0][li];
                    let au = 0.5
                        * ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                            + (p[2][0] - p[0][0]) * (p[0][1] - p[1][1]))
                            .abs();
                    if au > 0.0 {
                        d.push(au / wa);
                    }
                }
                d.sort_by(f32::total_cmp);
                if d.len() < 4 {
                    return f32::INFINITY;
                }
                let q = |f: f32| d[((d.len() as f32 - 1.0) * f).round() as usize];
                let (lo, hi) = (q(0.1), q(0.9));
                if lo > 0.0 { hi / lo } else { f32::INFINITY }
            };
            let sp_l = spread(std::slice::from_ref(&lscm_tri));
            let sp_p = spread(std::slice::from_ref(&planar_tri));
            per_tri = if sp_l <= sp_p { lscm_tri } else { planar_tri };
        } else {
            for &fi in g {
                per_tri.push([
                    proj_p(m.verts[m.tris[fi][0] as usize]),
                    proj_p(m.verts[m.tris[fi][1] as usize]),
                    proj_p(m.verts[m.tris[fi][2] as usize]),
                ]);
            }
        }
        coords.push(per_tri);
    }

    let mut bounds: Vec<([f32; 2], [f32; 2])> = Vec::with_capacity(group_list.len());
    for c in &coords {
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for tri in c {
            for p in tri {
                for d in 0..2 {
                    lo[d] = lo[d].min(p[d]);
                    hi[d] = hi[d].max(p[d]);
                }
            }
        }
        bounds.push((lo, hi));
    }

    let pad = 2usize;
    let mut scale = if texels_per_unit > 0.0 {
        texels_per_unit
    } else {
        let area: f32 = areas.iter().sum();
        (atlas_px as f32 * atlas_px as f32 * 0.6 / area.max(1e-9)).sqrt()
    }
    .max(1e-6);
    let mut rects: Vec<[usize; 4]> = vec![[0, 0, 0, 0]; group_list.len()];
    let mut packed = false;
    for _ in 0..24 {
        let mut order: Vec<usize> = (0..bounds.len()).collect();
        order.sort_by(|&a, &b| {
            let ha = ((bounds[a].1[1] - bounds[a].0[1]) * scale).ceil() as usize;
            let hb = ((bounds[b].1[1] - bounds[b].0[1]) * scale).ceil() as usize;
            hb.cmp(&ha)
        });
        rects = vec![[0, 0, 0, 0]; group_list.len()];
        let (mut cx, mut cy, mut shelf_h, mut ok) = (0usize, 0usize, 0usize, true);
        for &gi in &order {
            let (lo, hi) = bounds[gi];
            let w = (((hi[0] - lo[0]) * scale).ceil() as usize).max(1);
            let h = (((hi[1] - lo[1]) * scale).ceil() as usize).max(1);
            if cx + w + pad > atlas_px {
                cx = 0;
                cy += shelf_h + pad;
                shelf_h = 0;
            }
            if cy + h + pad > atlas_px {
                ok = false;
                break;
            }
            rects[gi] = [cx, cy, w, h];
            cx += w + pad;
            shelf_h = shelf_h.max(h);
        }
        if ok {
            packed = true;
            break;
        }
        scale *= 0.5;
    }
    if !packed {
        return Err("图集打包失败：减小 texels_per_unit 或增大 atlas_px".into());
    }

    let mut mean = [0.0f32; 3];
    for p in tex.chunks(3) {
        for c in 0..3 {
            mean[c] += p[c];
        }
    }
    let cnt = (tex.len() / 3) as f32;
    for mv in mean.iter_mut() {
        *mv /= cnt;
    }
    let (tlo, thi) = if per_class.is_some() {
        (
            tex.iter().cloned().fold(f32::INFINITY, f32::min),
            tex.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        )
    } else {
        (0.0, 1.0)
    };
    let mut img = Vec::with_capacity(atlas_px * atlas_px * 3);
    for _ in 0..atlas_px * atlas_px {
        img.extend_from_slice(&mean);
    }
    let mut uv = vec![[[0.0f32; 2]; 3]; n];
    let mut styled = vec![0usize; per_class.map(|p| p.styles.len()).unwrap_or(0)];
    let mut charts = Vec::with_capacity(group_list.len());
    for (gi, g) in group_list.iter().enumerate() {
        let (lo, _) = bounds[gi];
        let [rx, ry, _, _] = rects[gi];
        let to_px = |p: [f32; 2]| -> [f32; 2] {
            [
                rx as f32 + (p[0] - lo[0]) * scale,
                atlas_px as f32 - (ry as f32 + (p[1] - lo[1]) * scale),
            ]
        };
        for (li, &fi) in g.iter().enumerate() {
            for k in 0..3 {
                let p = to_px(coords[gi][li][k]);
                // 图集行从上往下、OBJ 的 v 从下往上 ⇒ v 翻转，否则采样落到 padding
                uv[fi][k] = [p[0] / atlas_px as f32, 1.0 - p[1] / atlas_px as f32];
            }
        }
        for (li, &fi) in g.iter().enumerate() {
            let src = coords[gi][li];
            let ps: Vec<[f32; 2]> = src.iter().map(|&p| to_px(p)).collect();
            let minx = (ps
                .iter()
                .map(|p| p[0])
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0)) as usize;
            let maxx = ((ps
                .iter()
                .map(|p| p[0])
                .fold(f32::MIN, f32::max)
                .ceil()
                .max(0.0)) as usize)
                .min(atlas_px - 1);
            let miny = (ps
                .iter()
                .map(|p| p[1])
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0)) as usize;
            let maxy = ((ps
                .iter()
                .map(|p| p[1])
                .fold(f32::MIN, f32::max)
                .ceil()
                .max(0.0)) as usize)
                .min(atlas_px - 1);
            let den = (ps[1][1] - ps[2][1]) * (ps[0][0] - ps[2][0])
                + (ps[2][0] - ps[1][0]) * (ps[0][1] - ps[2][1]);
            if den.abs() < 1e-9 {
                continue;
            }
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((ps[1][1] - ps[2][1]) * (px - ps[2][0])
                        + (ps[2][0] - ps[1][0]) * (py - ps[2][1]))
                        / den;
                    let w1 = ((ps[2][1] - ps[0][1]) * (px - ps[2][0])
                        + (ps[0][0] - ps[2][0]) * (py - ps[2][1]))
                        / den;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let u = w0 * src[0][0] + w1 * src[1][0] + w2 * src[2][0];
                    let v = w0 * src[0][1] + w1 * src[1][1] + w2 * src[2][1];
                    let mut c = sample(tex, tw, th, u, v);
                    if let Some(pc) = per_class {
                        let k = *pc.class.get(fi).unwrap_or(&0) as usize;
                        if let Some(st) = pc.styles.get(k) {
                            let before = c;
                            c = style_texel(c, st, mean, tlo, thi);
                            if c != before && k < styled.len() {
                                styled[k] += 1;
                            }
                        }
                    }
                    let i = (y * atlas_px + x) * 3;
                    img[i..i + 3].copy_from_slice(&c);
                }
            }
        }
        charts.push(Chart {
            tris: g.clone(),
            rect: rects[gi],
        });
    }
    for _ in 0..2 {
        let prev = img.clone();
        for y in 1..atlas_px - 1 {
            for x in 1..atlas_px - 1 {
                let i = (y * atlas_px + x) * 3;
                if prev[i..i + 3] != mean[..] {
                    continue;
                }
                let mut acc = [0.0f32; 3];
                let mut cnt2 = 0.0f32;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let j =
                        (((y as isize + dy) as usize) * atlas_px + (x as isize + dx) as usize) * 3;
                    if prev[j..j + 3] != mean[..] {
                        for c in 0..3 {
                            acc[c] += prev[j + c];
                        }
                        cnt2 += 1.0;
                    }
                }
                if cnt2 > 0.0 {
                    for c in 0..3 {
                        img[i + c] = acc[c] / cnt2;
                    }
                }
            }
        }
    }
    Ok(Atlas {
        img,
        w: atlas_px,
        h: atlas_px,
        styled_texels: styled,
        charts,
        uv,
    })
}

/// UV 质量统计：纹素密度 = UV 面积 × 图集面积 / 世界面积（texel²/单位²）。
///
/// 理想展开时所有三角形密度相同（`p95/p05` → 1）；平面投影在斜面上会被拉伸，
/// 这个比值就是拉伸的直接度量。
#[derive(Clone, Copy, Debug)]
pub struct UvStats {
    pub charts: usize,
    pub median: f32,
    pub p05: f32,
    pub p95: f32,
    /// p95/p05：越大表示密度越不均匀（拉伸越狠）。
    pub spread: f32,
}

pub fn uv_stats(m: &Mesh, a: &Atlas) -> UvStats {
    let mut d: Vec<f32> = Vec::with_capacity(m.tris.len());
    for (fi, t) in m.tris.iter().enumerate() {
        let wa = tri_area(m, t);
        if wa < 1e-12 {
            continue;
        }
        let p = a.uv[fi];
        let area_uv = 0.5
            * ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                + (p[2][0] - p[0][0]) * (p[0][1] - p[1][1]))
                .abs();
        let tx = area_uv * (a.w * a.h) as f32 / wa;
        if tx.is_finite() && tx > 0.0 {
            d.push(tx);
        }
    }
    d.sort_by(f32::total_cmp);
    let q = |f: f32| -> f32 {
        if d.is_empty() {
            return 0.0;
        }
        d[((d.len() as f32 - 1.0) * f).round() as usize]
    };
    let (p05, p95) = (q(0.05), q(0.95));
    UvStats {
        charts: a.charts.len(),
        median: q(0.5),
        p05,
        p95,
        spread: if p05 > 0.0 { p95 / p05 } else { f32::INFINITY },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Mesh {
        let v = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let t = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        Mesh { verts: v, tris: t }
    }

    #[test]
    fn atlas_uvs_are_normalized_and_charts_do_not_overlap() {
        let m = cube();
        let tex = vec![0.5f32; 32 * 32 * 3];
        let a = bake_atlas(&m, &tex, 32, 32, 256, 60.0, 45.0, false, None).unwrap();
        assert_eq!(a.uv.len(), m.tris.len());
        for tri in &a.uv {
            for p in tri {
                assert!(
                    (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]),
                    "UV 必须落在 [0,1]，实际 {p:?}"
                );
            }
        }
        // 图集矩形两两不重叠
        for i in 0..a.charts.len() {
            for j in i + 1..a.charts.len() {
                let (a1, b1) = (a.charts[i].rect, a.charts[j].rect);
                let ox = a1[0] < b1[0] + b1[2] && b1[0] < a1[0] + a1[2];
                let oy = a1[1] < b1[1] + b1[3] && b1[1] < a1[1] + a1[3];
                assert!(!(ox && oy), "chart 矩形重叠：{a1:?} vs {b1:?}");
            }
        }
        assert!(
            a.charts.len() >= 6,
            "立方体至少 6 个 chart，实际 {}",
            a.charts.len()
        );
    }

    #[test]
    fn atlas_bakes_a_solid_texture_into_solid_pixels() {
        let m = cube();
        let (tw, th) = (16usize, 16usize);
        let tex: Vec<f32> = (0..tw * th).flat_map(|_| [0.25, 0.5, 0.75]).collect();
        let a = bake_atlas(&m, &tex, tw, th, 128, 40.0, 45.0, false, None).unwrap();
        // 所有被覆盖的纹素都必须等于源色（padding 处是 0，用 >0 判定被覆盖）
        let mut covered = 0usize;
        for px in a.img.chunks(3) {
            if px[0] > 0.0 || px[1] > 0.0 || px[2] > 0.0 {
                covered += 1;
                assert!(
                    (px[0] - 0.25).abs() < 1e-6
                        && (px[1] - 0.5).abs() < 1e-6
                        && (px[2] - 0.75).abs() < 1e-6,
                    "烘出来的颜色应等于源色，实际 {px:?}"
                );
            }
        }
        assert!(covered > 500, "图集应被覆盖，实际 {covered} 个纹素");
    }

    #[test]
    fn flat_surface_has_uniform_texel_density() {
        // 两个直角三角组成的平面：同一 chart、同一投影 ⇒ 密度应几乎相同
        let m = Mesh {
            verts: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 2.0, 0.0],
                [0.0, 2.0, 0.0],
            ],
            tris: vec![[0, 1, 2], [0, 2, 3]],
        };
        let tex = vec![0.5f32; 8 * 8 * 3];
        let a = bake_atlas(&m, &tex, 8, 8, 128, 50.0, 45.0, false, None).unwrap();
        let st = uv_stats(&m, &a);
        assert_eq!(st.charts, 1, "共面应只出一个 chart");
        assert!(
            (st.spread - 1.0).abs() < 0.02,
            "共面密度应均匀，实际 spread {:.3}",
            st.spread
        );
    }

    fn cylinder_patch(nu: usize, nv: usize) -> Mesh {
        // 半圆柱面（弧 180°、半径 1、高 1）：平面投影会在两侧压缩，LSCM 应更均匀
        let mut verts = Vec::new();
        for j in 0..=nv {
            for i in 0..=nu {
                let th = std::f32::consts::PI * i as f32 / nu as f32;
                verts.push([th.cos(), j as f32 / nv as f32, th.sin()]);
            }
        }
        let mut tris = Vec::new();
        for j in 0..nv {
            for i in 0..nu {
                let a = (j * (nu + 1) + i) as u32;
                let b = (j * (nu + 1) + i + 1) as u32;
                let c = ((j + 1) * (nu + 1) + i) as u32;
                let d = ((j + 1) * (nu + 1) + i + 1) as u32;
                tris.push([a, b, c]);
                tris.push([b, d, c]);
            }
        }
        Mesh { verts, tris }
    }

    #[test]
    fn lscm_flattens_a_curved_patch_better_than_planar_projection() {
        let m = cylinder_patch(32, 8);
        let tex = vec![0.5f32; 8 * 8 * 3];
        let planar = bake_atlas(&m, &tex, 8, 8, 512, 0.0, 89.0, false, None).unwrap();
        let lscm = bake_atlas(&m, &tex, 8, 8, 512, 0.0, 89.0, true, None).unwrap();
        let sp = uv_stats(&m, &planar);
        let sl = uv_stats(&m, &lscm);
        assert_eq!(
            sp.charts, 1,
            "半圆柱应是一个 chart（阈值 89°），实际 {}",
            sp.charts
        );
        assert_eq!(sl.charts, 1);
        assert!(
            sl.spread < 3.0,
            "LSCM 后密度应接近均匀，实际 spread {:.2}",
            sl.spread
        );
        assert!(
            sl.spread < sp.spread * 0.6,
            "LSCM 应显著优于平面投影：planar {:.2} vs lscm {:.2}",
            sp.spread,
            sl.spread
        );
    }

    #[test]
    fn per_class_styles_only_affect_their_own_triangles() {
        // 同一平面两个三角形：第 0 个强制灰度，第 1 个不动
        let m = Mesh {
            verts: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            tris: vec![[0, 1, 2], [0, 2, 3]],
        };
        // 彩色源（R 通道随 x 变化）
        let (tw, th) = (32usize, 32usize);
        let tex: Vec<f32> = (0..tw * th)
            .flat_map(|i| {
                let x = (i % tw) as f32 / tw as f32;
                [x, 0.5, 0.5]
            })
            .collect();
        let gray = crate::tex::Style {
            sat: 0.0,
            ..Default::default()
        };
        let id = crate::tex::Style::default();
        let styles = [gray, id];
        let class = [0u8, 1u8];
        let a = bake_atlas(
            &m,
            &tex,
            tw,
            th,
            128,
            50.0,
            45.0,
            false,
            Some(&PerClass {
                class: &class,
                styles: &styles,
            }),
        )
        .unwrap();
        let at = |fi: usize| -> [f32; 3] {
            // 取该三角形 UV 质心处的图集像素
            let u = (a.uv[fi][0][0] + a.uv[fi][1][0] + a.uv[fi][2][0]) / 3.0;
            let v = (a.uv[fi][0][1] + a.uv[fi][1][1] + a.uv[fi][2][1]) / 3.0;
            let x = (u * a.w as f32) as usize;
            let y = ((1.0 - v) * a.h as f32) as usize;
            let i = (y.min(a.h - 1) * a.w + x.min(a.w - 1)) * 3;
            [a.img[i], a.img[i + 1], a.img[i + 2]]
        };
        let c0 = at(0);
        let c1 = at(1);
        assert!(
            (c0[0] - c0[1]).abs() < 0.02 && (c0[1] - c0[2]).abs() < 0.02,
            "类别 0 应为灰度，实际 {c0:?}"
        );
        assert!(
            (c1[0] - c1[1]).abs() > 0.05,
            "类别 1 应保留彩色，实际 {c1:?}"
        );
        // 精确判据：只有类别 0 的纹素被改写
        assert!(a.styled_texels[0] > 0, "类别 0 应有纹素被改写");
        assert_eq!(a.styled_texels[1], 0, "类别 1（恒等）不应改写任何纹素");
    }

    #[test]
    fn atlas_is_deterministic() {
        let m = cube();
        let tex = vec![0.3f32; 16 * 16 * 3];
        let a = bake_atlas(&m, &tex, 16, 16, 128, 40.0, 45.0, false, None).unwrap();
        let b = bake_atlas(&m, &tex, 16, 16, 128, 40.0, 45.0, false, None).unwrap();
        assert_eq!(a.uv, b.uv);
        assert_eq!(a.img, b.img);
        assert_eq!(a.charts.len(), b.charts.len());
    }
}
