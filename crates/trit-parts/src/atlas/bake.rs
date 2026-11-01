use std::collections::HashMap;

use crate::mesh::Mesh;

use super::charts::{
    Atlas, Chart, PerClass, find, lscm_chart, norm, sample, style_texel, tri_area, tri_normal,
};

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
