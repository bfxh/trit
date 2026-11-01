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

/// 切 chart + 打包 + 烘焙。
///
/// chart 用**法线相似度**聚类（相邻面法线夹角 <= `normal_deg` 才合并），
/// 每个 chart 用面积加权平均法线建切平面做正交投影——光滑区域 chart 很大，
/// 不会像「按主轴硬切」那样在噪声表面上碎成一堆几像素的小岛。
///
/// `texels_per_unit <= 0` 时按总面积自动定密度（目标填充 60%）；放不下会减半重排。
pub fn bake_atlas(
    m: &Mesh,
    tex: &[f32],
    tw: usize,
    th: usize,
    atlas_px: usize,
    texels_per_unit: f32,
    normal_deg: f32,
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

    let frames: Vec<([f32; 3], [f32; 3], [f32; 3])> = group_list
        .iter()
        .map(|g| {
            let mut acc = [0.0f32; 3];
            for &fi in g {
                for c in 0..3 {
                    acc[c] += nrm[fi][c] * areas[fi].max(1e-12);
                }
            }
            let l = norm(acc).max(1e-12);
            let nv = [acc[0] / l, acc[1] / l, acc[2] / l];
            let up = if nv[1].abs() < 0.9 {
                [0.0, 1.0, 0.0]
            } else {
                [1.0, 0.0, 0.0]
            };
            let mut u = [
                up[1] * nv[2] - up[2] * nv[1],
                up[2] * nv[0] - up[0] * nv[2],
                up[0] * nv[1] - up[1] * nv[0],
            ];
            let lu = norm(u).max(1e-12);
            u = [u[0] / lu, u[1] / lu, u[2] / lu];
            let v = [
                nv[1] * u[2] - nv[2] * u[1],
                nv[2] * u[0] - nv[0] * u[2],
                nv[0] * u[1] - nv[1] * u[0],
            ];
            (nv, u, v)
        })
        .collect();
    let proj = |fi: usize, k: usize, fr: ([f32; 3], [f32; 3], [f32; 3])| -> [f32; 2] {
        let p = m.verts[m.tris[fi][k] as usize];
        [
            p[0] * fr.1[0] + p[1] * fr.1[1] + p[2] * fr.1[2],
            p[0] * fr.2[0] + p[1] * fr.2[1] + p[2] * fr.2[2],
        ]
    };

    let mut bounds: Vec<([f32; 2], [f32; 2])> = Vec::with_capacity(group_list.len());
    for (gi, g) in group_list.iter().enumerate() {
        let fr = frames[gi];
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for &fi in g {
            for k in 0..3 {
                let p = proj(fi, k, fr);
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
    for m in mean.iter_mut() {
        *m /= cnt;
    }
    let mut img = Vec::with_capacity(atlas_px * atlas_px * 3);
    for _ in 0..atlas_px * atlas_px {
        img.extend_from_slice(&mean);
    }
    let mut uv = vec![[[0.0f32; 2]; 3]; n];
    let mut charts = Vec::with_capacity(group_list.len());
    for (gi, g) in group_list.iter().enumerate() {
        let (lo, _) = bounds[gi];
        let fr = frames[gi];
        let [rx, ry, _, _] = rects[gi];
        let to_px = |p: [f32; 2]| -> [f32; 2] {
            [
                rx as f32 + (p[0] - lo[0]) * scale,
                atlas_px as f32 - (ry as f32 + (p[1] - lo[1]) * scale),
            ]
        };
        for &fi in g {
            for (k, slot) in uv[fi].iter_mut().enumerate() {
                let p = to_px(proj(fi, k, fr));
                // 图集行是从上往下、OBJ 的 v 是从下往上 ⇒ v 要翻转，否则采样会落到 padding
                *slot = [p[0] / atlas_px as f32, 1.0 - p[1] / atlas_px as f32];
            }
        }
        for &fi in g {
            let src: Vec<[f32; 2]> = (0..3).map(|k| proj(fi, k, fr)).collect();
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
                    let c = sample(tex, tw, th, u, v);
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
                let mut cnt = 0.0f32;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let j =
                        (((y as isize + dy) as usize) * atlas_px + (x as isize + dx) as usize) * 3;
                    if prev[j..j + 3] != mean[..] {
                        for c in 0..3 {
                            acc[c] += prev[j + c];
                        }
                        cnt += 1.0;
                    }
                }
                if cnt > 0.0 {
                    for c in 0..3 {
                        img[i + c] = acc[c] / cnt;
                    }
                }
            }
        }
    }
    Ok(Atlas {
        img,
        w: atlas_px,
        h: atlas_px,
        charts,
        uv,
    })
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
        let a = bake_atlas(&m, &tex, 32, 32, 256, 60.0, 45.0).unwrap();
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
        let a = bake_atlas(&m, &tex, tw, th, 128, 40.0, 45.0).unwrap();
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
    fn atlas_is_deterministic() {
        let m = cube();
        let tex = vec![0.3f32; 16 * 16 * 3];
        let a = bake_atlas(&m, &tex, 16, 16, 128, 40.0, 45.0).unwrap();
        let b = bake_atlas(&m, &tex, 16, 16, 128, 40.0, 45.0).unwrap();
        assert_eq!(a.uv, b.uv);
        assert_eq!(a.img, b.img);
        assert_eq!(a.charts.len(), b.charts.len());
    }
}
