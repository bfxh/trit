use std::path::Path;

use super::build::{Mesh, boundary_edges};
use super::morph::multiface_edges;

/// 清理统计（前/后对照）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanStats {
    pub verts_before: usize,
    pub faces_before: usize,
    pub degenerate: usize,
    pub duplicate: usize,
    pub nonmanifold_before: usize,
    pub boundary_before: usize,
    pub verts_after: usize,
    pub faces_after: usize,
    pub nonmanifold_after: usize,
    pub boundary_after: usize,
}

fn face_area2(m: &Mesh, t: &[u32; 3]) -> f32 {
    let a = m.verts[t[0] as usize];
    let b = m.verts[t[1] as usize];
    let c = m.verts[t[2] as usize];
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    n[0] * n[0] + n[1] * n[1] + n[2] * n[2]
}

/// 梅沙清理：丢退化面、去重复面、按一环连通分量分裂非流形顶点。
///
/// 分裂规则：同一顶点周围的面，只有**共享一条恰好 2 面的边**才算连通；共边 ≥3 的面之间不连通
/// ⇒ 两张仅在一条边上相切的曲面会各自拿到一份顶点副本（切片/薄壁处常见），分裂后该边不再被 >2 面共用。
/// 副作用诚实记录在 [`CleanStats`] 里：分裂可能把「相切」变成两条边界边（不再闭合）。
pub fn clean(m: &Mesh) -> (Mesh, CleanStats) {
    use std::collections::{HashMap, HashSet};
    let mut st = CleanStats {
        verts_before: m.verts.len(),
        faces_before: m.tris.len(),
        nonmanifold_before: multiface_edges(m),
        boundary_before: boundary_edges(m),
        ..Default::default()
    };
    // 1) 丢退化面（重复索引或面积为零）+ 去重复面（顶点集合相同）
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(m.tris.len());
    for t in &m.tris {
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] || face_area2(m, t) < 1e-20 {
            st.degenerate += 1;
            continue;
        }
        let mut k = *t;
        k.sort_unstable();
        if !seen.insert(k) {
            st.duplicate += 1;
            continue;
        }
        tris.push(*t);
    }
    // 2) 边 → 面（只在当前保留的面里建）
    let mut e2f: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (fi, t) in tris.iter().enumerate() {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = if a < b { (a, b) } else { (b, a) };
            e2f.entry(e).or_default().push(fi);
        }
    }
    let mut v2f: HashMap<u32, Vec<usize>> = HashMap::new();
    for (fi, t) in tris.iter().enumerate() {
        for &v in t {
            v2f.entry(v).or_default().push(fi);
        }
    }
    // 3) 每个顶点：按「共享流形边」做并查集，非首分量拿副本
    let mut verts = m.verts.clone();
    let mut new_tris = tris.clone();
    let mut split = 0usize;
    let mut keys: Vec<u32> = v2f.keys().copied().collect();
    keys.sort_unstable();
    for v in keys {
        let fs = &v2f[&v];
        if fs.len() < 2 {
            continue;
        }
        let mut parent: Vec<usize> = (0..fs.len()).collect();
        fn find(p: &mut [usize], x: usize) -> usize {
            if p[x] != x {
                let r = find(p, p[x]);
                p[x] = r;
            }
            p[x]
        }
        let local: HashMap<usize, usize> = fs.iter().enumerate().map(|(i, &f)| (f, i)).collect();
        // 只走该顶点的邻面（O(度)），不要扫全部三角形（O(V·F) 在 10 万面上会卡死）
        for (i, &fi) in fs.iter().enumerate() {
            let t = &tris[fi];
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                if a != v && b != v {
                    continue;
                }
                let e = if a < b { (a, b) } else { (b, a) };
                if let Some(faces) = e2f.get(&e)
                    && faces.len() == 2
                {
                    let j = local[&faces[0]];
                    if j == i {
                        continue;
                    }
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    if ri != rj {
                        parent[ri] = rj;
                    }
                }
            }
        }
        // 每个根（按首次出现顺序）一个顶点：第一个用原 id，其余各建一份副本
        let roots: Vec<usize> = (0..fs.len()).map(|i| find(&mut parent, i)).collect();
        let mut root_id: HashMap<usize, u32> = HashMap::new();
        for (i, &r) in roots.iter().enumerate() {
            let id = *root_id.entry(r).or_insert_with(|| {
                if i == 0 {
                    v
                } else {
                    split += 1;
                    verts.push(m.verts[v as usize]);
                    (verts.len() - 1) as u32
                }
            });
            if id != v {
                let fi = fs[i];
                for c in new_tris[fi].iter_mut() {
                    if *c == v {
                        *c = id;
                    }
                }
            }
        }
    }
    // 4) 压缩未引用顶点
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut out_v: Vec<[f32; 3]> = Vec::with_capacity(verts.len());
    for t in &new_tris {
        for &v in t {
            remap.entry(v).or_insert_with(|| {
                out_v.push(verts[v as usize]);
                (out_v.len() - 1) as u32
            });
        }
    }
    let out_t: Vec<[u32; 3]> = new_tris
        .iter()
        .map(|t| [remap[&t[0]], remap[&t[1]], remap[&t[2]]])
        .collect();
    let out = Mesh {
        verts: out_v,
        tris: out_t,
    };
    st.verts_after = out.verts.len();
    st.faces_after = out.tris.len();
    st.nonmanifold_after = multiface_edges(&out);
    st.boundary_after = boundary_edges(&out);
    let _ = split;
    (out, st)
}

/// 写带顶点颜色的 ASCII PLY（`None` = 未观测，写成灰色 128）。
pub fn write_ply_rgb(path: &Path, m: &Mesh, colors: &[Option<[u8; 3]>]) -> Result<(), String> {
    use std::fmt::Write as _;
    if colors.len() != m.verts.len() {
        return Err(format!(
            "颜色数 {} != 顶点数 {}",
            colors.len(),
            m.verts.len()
        ));
    }
    let mut s = String::new();
    writeln!(
        s,
        "ply
format ascii 1.0
element vertex {}
property float x
property float y
property float z
property uchar red
property uchar green
property uchar blue
element face {}
property list uchar int vertex_indices
end_header",
        m.verts.len(),
        m.tris.len()
    )
    .map_err(|e| e.to_string())?;
    for (v, c) in m.verts.iter().zip(colors) {
        let [r, g, b] = c.unwrap_or([128, 128, 128]);
        writeln!(s, "{} {} {} {r} {g} {b}", v[0], v[1], v[2]).map_err(|e| e.to_string())?;
    }
    for t in &m.tris {
        writeln!(s, "3 {} {} {}", t[0], t[1], t[2]).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

/// 写 ASCII PLY。
pub fn write_ply(path: &Path, m: &Mesh) -> Result<(), String> {
    use std::fmt::Write as _;
    let mut s = String::new();
    writeln!(s, "ply\nformat ascii 1.0\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nelement face {}\nproperty list uchar int vertex_indices\nend_header", m.verts.len(), m.tris.len()).map_err(|e| e.to_string())?;
    for v in &m.verts {
        writeln!(s, "{} {} {}", v[0], v[1], v[2]).map_err(|e| e.to_string())?;
    }
    for t in &m.tris {
        writeln!(s, "3 {} {} {}", t[0], t[1], t[2]).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}
