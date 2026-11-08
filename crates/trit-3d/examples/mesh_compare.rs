//! 真实 mesh 验收：Chamfer + 法线一致性 + 体积/面积（不只 χ）。
//!
//! 法线用 GT 三角形哈希（质心格 + 27 邻域）找最近三角形，避免采样点法线的匹配误差。
//!
//! 运行：`cargo run --release -q -p trit-3d --example mesh_compare -- <gt.obj> <recon.obj>`

use std::collections::HashMap;

use trit_3d::mesh::{TriMesh, chamfer_points};

fn tri_normal(m: &TriMesh, t: [u32; 3]) -> [f32; 3] {
    let a = m.verts[t[0] as usize];
    let b = m.verts[t[1] as usize];
    let c = m.verts[t[2] as usize];
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
    [n[0] / l, n[1] / l, n[2] / l]
}

fn centroid(m: &TriMesh, t: [u32; 3]) -> [f32; 3] {
    [
        (m.verts[t[0] as usize][0] + m.verts[t[1] as usize][0] + m.verts[t[2] as usize][0]) / 3.0,
        (m.verts[t[0] as usize][1] + m.verts[t[1] as usize][1] + m.verts[t[2] as usize][1]) / 3.0,
        (m.verts[t[0] as usize][2] + m.verts[t[1] as usize][2] + m.verts[t[2] as usize][2]) / 3.0,
    ]
}

fn key(c: [f32; 3], cell: f32) -> (i32, i32, i32) {
    (
        (c[0] / cell).floor() as i32,
        (c[1] / cell).floor() as i32,
        (c[2] / cell).floor() as i32,
    )
}

fn main() {
    let gt_path = std::env::args().nth(1).expect("用法: mesh_compare <gt.obj> <recon.obj>");
    let rc_path = std::env::args().nth(2).expect("用法: mesh_compare <gt.obj> <recon.obj>");
    let gt = TriMesh::from_obj(std::path::Path::new(&gt_path)).expect("读 GT");
    let rc = TriMesh::from_obj(std::path::Path::new(&rc_path)).expect("读重建");

    // Chamfer 用采样点。
    let frames = gt.surface_frames(32768);
    let gt_pts: Vec<[f32; 3]> = frames.iter().map(|(p, _, _)| *p).collect();
    let rc_pts = rc.surface_points(1024);
    let chamfer = chamfer_points(&rc_pts, &gt_pts);

    // GT 三角形哈希：质心格 + 27 邻域，找最近三角形法线。
    let cell = 0.05f32;
    let mut grid: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
    let mut cent: Vec<[f32; 3]> = Vec::with_capacity(gt.tris.len());
    for (i, t) in gt.tris.iter().enumerate() {
        let c = centroid(&gt, *t);
        cent.push(c);
        grid.entry(key(c, cell)).or_default().push(i);
    }

    let step = (rc.tris.len() / 512).max(1);
    let mut dot = 0.0f32;
    let mut count = 0usize;
    for t in rc.tris.iter().step_by(step) {
        let c = centroid(&rc, *t);
        let n = tri_normal(&rc, *t);
        let base = key(c, cell);
        let mut best = f32::INFINITY;
        let mut bi = usize::MAX;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let k = (base.0 + dx, base.1 + dy, base.2 + dz);
                    if let Some(list) = grid.get(&k) {
                        for &j in list {
                            let d = (cent[j][0] - c[0]).powi(2)
                                + (cent[j][1] - c[1]).powi(2)
                                + (cent[j][2] - c[2]).powi(2);
                            if d < best {
                                best = d;
                                bi = j;
                            }
                        }
                    }
                }
            }
        }
        if bi == usize::MAX {
            continue;
        }
        let g = tri_normal(&gt, gt.tris[bi]);
        dot += (n[0] * g[0] + n[1] * g[1] + n[2] * g[2]).abs();
        count += 1;
    }
    dot /= count.max(1) as f32;

    let aa = gt.surface_area();
    let ab = rc.surface_area();
    let va = gt.signed_volume().abs();
    let vb = rc.signed_volume().abs();
    println!(
        "{{\"gt_tris\":{},\"recon_tris\":{},\"chamfer2\":{:.6},\"normal_dot\":{:.4},\"volume_gt\":{:.6},\"volume_recon\":{:.6},\"volume_ratio\":{:.4},\"area_gt\":{:.6},\"area_recon\":{:.6},\"area_ratio\":{:.4}}}",
        gt.tris.len(),
        rc.tris.len(),
        chamfer,
        dot,
        va,
        vb,
        if va > 1e-9 { vb / va } else { 0.0 },
        aa,
        ab,
        if aa > 1e-9 { ab / aa } else { 0.0 }
    );
}
