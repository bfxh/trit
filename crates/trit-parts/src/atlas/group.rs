//! chart 分组的两种策略（被 `bake` 调用）：
//! - union：相邻面法线夹角 ≤ 阈值即合并（union-find 旧行为，链式合并会传递性漂移）；
//! - anchored：锚定法线贪心生长——以 chart 已收面元的面积加权均值法线为锚，新面与锚
//!   夹角 ≤ 阈值才收，被拒的面留给后续 chart，遏制漂移、降低曲面 UV 密度 spread。

use std::collections::{HashMap, VecDeque};

use super::charts::{find, norm};

/// 边 → 相邻面表（键 `(min,max)`；值按面号升序——面号按遍历序 push，天然升序）。
pub(crate) fn build_edge_faces(tris: &[[u32; 3]]) -> HashMap<(u32, u32), Vec<usize>> {
    let mut e2f: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (fi, t) in tris.iter().enumerate() {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = if a < b { (a, b) } else { (b, a) };
            e2f.entry(e).or_default().push(fi);
        }
    }
    e2f
}

/// 旧行为：相邻面法线夹角 <= `normal_deg` 即 union-find 合并（传递性漂移：A~B、B~C
/// 链式合并后 A⊥C 也在同一 chart）。groups 按 `sort_by_key(|g| g[0])` 排序，与原 bake 一致。
pub(crate) fn union_groups(
    tris: &[[u32; 3]],
    nrm: &[[f32; 3]],
    normal_deg: f32,
) -> Vec<Vec<usize>> {
    let n = tris.len();
    let e2f = build_edge_faces(tris);
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
    group_list
}

/// 面邻接表：沿每面固定边序 `[(0,1),(1,2),(2,0)]` 展开，桶内面号升序，同一邻居去重。
fn face_neighbors(tris: &[[u32; 3]]) -> Vec<Vec<usize>> {
    let e2f = build_edge_faces(tris);
    let mut nbrs: Vec<Vec<usize>> = vec![Vec::new(); tris.len()];
    for (fi, t) in tris.iter().enumerate() {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = if a < b { (a, b) } else { (b, a) };
            if let Some(fs) = e2f.get(&e) {
                for &nf in fs {
                    if nf != fi && !nbrs[fi].contains(&nf) {
                        nbrs[fi].push(nf);
                    }
                }
            }
        }
    }
    nbrs
}

/// 锚定法线贪心生长：种子按面号升序；每个 chart 从种子 BFS（VecDeque，邻居沿固定边序
/// 展开）。面 `nf` 加入当前 chart 当且仅当 `dot(nrm[nf], mean_hat) >= cos(normal_deg)`，
/// 其中 `mean_hat` 是 chart 已收面元的面积加权均值法线（逐次接收后增量更新，使用前
/// 归一化）。被拒的面保持未分配，可被后续 chart 收走；返回按 `g[0]` 升序。
pub(crate) fn anchored_groups(
    tris: &[[u32; 3]],
    nrm: &[[f32; 3]],
    areas: &[f32],
    normal_deg: f32,
) -> Vec<Vec<usize>> {
    let n = tris.len();
    let nbrs = face_neighbors(tris);
    let cos_thr = normal_deg.to_radians().cos();
    let mut assigned = vec![false; n];
    let mut seen = vec![usize::MAX; n]; // 该面最近被哪个 chart 入队过（chart 内只测一次）
    let mut out: Vec<Vec<usize>> = Vec::new();
    for seed in 0..n {
        if assigned[seed] {
            continue;
        }
        let cid = out.len();
        let mut acc = [0.0f32; 3];
        let mut mean_hat = nrm[seed]; // 种子的均值法线即其自身（nrm 已归一化）
        let mut chart = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(seed);
        seen[seed] = cid;
        while let Some(fi) = queue.pop_front() {
            let d = nrm[fi][0] * mean_hat[0] + nrm[fi][1] * mean_hat[1] + nrm[fi][2] * mean_hat[2];
            if d < cos_thr {
                continue; // 被拒：保持未分配，也不扩展 frontier
            }
            chart.push(fi);
            assigned[fi] = true;
            // 面积加权均值法线增量更新：acc 累加后按自身模长归一化（方向即加权均值方向）
            let w = areas.get(fi).copied().unwrap_or(0.0).max(1e-12);
            for c in 0..3 {
                acc[c] += nrm[fi][c] * w;
            }
            let l = norm(acc).max(1e-12);
            mean_hat = [acc[0] / l, acc[1] / l, acc[2] / l];
            for &nf in &nbrs[fi] {
                if !assigned[nf] && seen[nf] != cid {
                    seen[nf] = cid;
                    queue.push_back(nf);
                }
            }
        }
        out.push(chart);
    }
    out.sort_by_key(|g| g[0]);
    out
}

/// 分派：`anchor=false` 走 union（逐位旧行为），`true` 走锚定法线贪心生长。
pub(crate) fn chart_groups(
    tris: &[[u32; 3]],
    nrm: &[[f32; 3]],
    areas: &[f32],
    normal_deg: f32,
    anchor: bool,
) -> Vec<Vec<usize>> {
    if anchor {
        anchored_groups(tris, nrm, areas, normal_deg)
    } else {
        union_groups(tris, nrm, normal_deg)
    }
}

#[cfg(test)]
mod tests {
    use super::super::charts::{norm, tri_area, tri_normal};
    use super::*;
    use crate::mesh::Mesh;

    /// 参数化带面 `p(i,s) = [i, s·cos(iθ), s·sin(iθ)]`：s=0 沿 x 轴、s=1 在单位圆上，
    /// 每列一个 quad 拆两个三角形，绕向一致（法线随列连续转动）；相邻列法线差 ≈ θ，
    /// 全程单调漂移 n_cols·θ。
    fn sweep_surface(n_cols: usize, theta_deg: f32) -> Mesh {
        let th = theta_deg.to_radians();
        let mut verts = Vec::new();
        for i in 0..=n_cols {
            let a = i as f32 * th;
            verts.push([i as f32, 0.0, 0.0]);
            verts.push([i as f32, a.cos(), a.sin()]);
        }
        let vi = |i: usize, s: usize| (i * 2 + s) as u32;
        let mut tris = Vec::new();
        for i in 0..n_cols {
            let (a, b, c, d) = (vi(i, 0), vi(i, 1), vi(i + 1, 1), vi(i + 1, 0));
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
        Mesh { verts, tris }
    }

    /// x-y 平面网格，全部三角形法线 +z。
    fn flat_plane(w: usize, h: usize) -> Mesh {
        let mut verts = Vec::new();
        for y in 0..=h {
            for x in 0..=w {
                verts.push([x as f32, y as f32, 0.0]);
            }
        }
        let vi = |x: usize, y: usize| (y * (w + 1) + x) as u32;
        let mut tris = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let (a, b, c, d) = (vi(x, y), vi(x + 1, y), vi(x + 1, y + 1), vi(x, y + 1));
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            }
        }
        Mesh { verts, tris }
    }

    /// 与 bake 同口径的面法线（归一化）与面积。
    fn face_data(m: &Mesh) -> (Vec<[f32; 3]>, Vec<f32>) {
        let mut nrm = Vec::with_capacity(m.tris.len());
        let mut areas = Vec::with_capacity(m.tris.len());
        for t in &m.tris {
            let v = tri_normal(m, t);
            let l = norm(v).max(1e-12);
            nrm.push([v[0] / l, v[1] / l, v[2] / l]);
            areas.push(tri_area(m, t));
        }
        (nrm, areas)
    }

    #[test]
    fn union_merges_chain_on_monotone_twist() {
        let m = sweep_surface(12, 10.0);
        let (nrm, _) = face_data(&m);
        assert_eq!(union_groups(&m.tris, &nrm, 20.0).len(), 1);
    }

    #[test]
    fn anchor_splits_where_union_drifts() {
        let m = sweep_surface(12, 10.0);
        let (nrm, areas) = face_data(&m);
        let g20 = anchored_groups(&m.tris, &nrm, &areas, 20.0);
        assert!(g20.len() >= 2, "锚定 20° 应 ≥2 个 chart");
        let g90 = anchored_groups(&m.tris, &nrm, &areas, 90.0);
        assert_eq!(g90.len(), 1, "锚定 90° 应 1 个 chart");
    }

    #[test]
    fn flat_plane_single_chart_both_modes() {
        let m = flat_plane(3, 2);
        let (nrm, areas) = face_data(&m);
        for deg in [20.0f32, 90.0f32] {
            let u = union_groups(&m.tris, &nrm, deg).len();
            let a = anchored_groups(&m.tris, &nrm, &areas, deg).len();
            assert_eq!((u, a), (1, 1), "deg {deg} 应 1 chart");
        }
    }

    #[test]
    fn grouping_is_deterministic_and_covers_all_faces() {
        let m = sweep_surface(12, 10.0);
        let (nrm, areas) = face_data(&m);
        let u1 = union_groups(&m.tris, &nrm, 20.0);
        let a1 = anchored_groups(&m.tris, &nrm, &areas, 20.0);
        let u2 = union_groups(&m.tris, &nrm, 20.0);
        let a2 = anchored_groups(&m.tris, &nrm, &areas, 20.0);
        assert_eq!(u1, u2, "union 两次一致");
        assert_eq!(a1, a2, "anchor 两次一致");
        let d0 = chart_groups(&m.tris, &nrm, &areas, 20.0, false);
        let d1 = chart_groups(&m.tris, &nrm, &areas, 20.0, true);
        assert_eq!(d0, u1, "分派=union");
        assert_eq!(d1, a1, "分派=anchored");
        for groups in [&u1, &a1] {
            let mut all: Vec<usize> = groups.iter().flatten().copied().collect();
            all.sort_unstable();
            let expect: Vec<usize> = (0..m.tris.len()).collect();
            assert_eq!(all, expect, "每面恰一次");
            assert!(
                groups.windows(2).all(|w| w[0][0] < w[1][0]),
                "chart 按 g[0] 升序"
            );
        }
    }

    #[test]
    fn anchor_reduces_uv_spread_on_curved_surface() {
        let m = sweep_surface(12, 10.0);
        let (tw, th) = (32usize, 32usize);
        let tex = vec![0.5f32; tw * th * 3];
        let bake = |anchor: bool| {
            crate::atlas::bake_atlas_ex(&m, &tex, tw, th, 256, 20.0, 20.0, false, anchor, None)
                .unwrap()
        };
        let (u, a) = (bake(false), bake(true));
        let su = crate::atlas::uv_stats(&m, &u);
        let sa = crate::atlas::uv_stats(&m, &a);
        assert!(a.charts.len() > u.charts.len(), "锚定应切出更多 chart");
        assert!(sa.spread < su.spread, "锚定应降低 spread");
        assert!(sa.spread < 1.3, "锚定 spread 应 <1.3");
    }
}
