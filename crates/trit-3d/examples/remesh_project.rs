//! 投影 remesh（ARAP 的简化前身）：把重建 mesh 的顶点投影到 GT 最近三角形，
//! 再可选做 Laplacian 正则化；拓扑不变，只动顶点。
//!
//! 运行：`cargo run --release -q -p trit-3d --example remesh_project -- <gt.obj> <recon.obj> <out.obj> [iters] [lambda]`

use std::collections::HashMap;

use trit_3d::mesh::TriMesh;

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Ericson 的最近点（点在三角形上）。
fn closest_on_tri(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return add(a, scale(ab, v));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return add(a, scale(ac, w));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), w));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    add(a, add(scale(ab, v), scale(ac, w)))
}

fn key(p: [f32; 3], cell: f32) -> (i32, i32, i32) {
    (
        (p[0] / cell).floor() as i32,
        (p[1] / cell).floor() as i32,
        (p[2] / cell).floor() as i32,
    )
}

fn main() {
    let gt_path = std::env::args().nth(1).expect("用法: remesh_project <gt.obj> <recon.obj> <out.obj> [iters] [lambda]");
    let rc_path = std::env::args().nth(2).expect("用法: remesh_project <gt.obj> <recon.obj> <out.obj> [iters] [lambda]");
    let out_path = std::env::args().nth(3).expect("用法: remesh_project <gt.obj> <recon.obj> <out.obj> [iters] [lambda]");
    let iters: usize = std::env::args()
        .nth(4)
        .unwrap_or_else(|| "1".to_string())
        .parse()
        .expect("iters");
    let lambda: f32 = std::env::args()
        .nth(5)
        .unwrap_or_else(|| "0.6".to_string())
        .parse()
        .expect("lambda");

    let gt = TriMesh::from_obj(std::path::Path::new(&gt_path)).expect("读 GT");
    let rc = TriMesh::from_obj(std::path::Path::new(&rc_path)).expect("读重建");

    // GT 三角形质心哈希。
    let cell = 0.05f32;
    let mut grid: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
    let mut cent: Vec<[f32; 3]> = Vec::with_capacity(gt.tris.len());
    for (i, t) in gt.tris.iter().enumerate() {
        let c = [
            (gt.verts[t[0] as usize][0] + gt.verts[t[1] as usize][0] + gt.verts[t[2] as usize][0]) / 3.0,
            (gt.verts[t[0] as usize][1] + gt.verts[t[1] as usize][1] + gt.verts[t[2] as usize][1]) / 3.0,
            (gt.verts[t[0] as usize][2] + gt.verts[t[1] as usize][2] + gt.verts[t[2] as usize][2]) / 3.0,
        ];
        cent.push(c);
        grid.entry(key(c, cell)).or_default().push(i);
    }

    let mut verts = rc.verts.clone();
    for _ in 0..iters {
        let old = verts.clone();
        for i in 0..verts.len() {
            let p = old[i];
            let base = key(p, cell);
            let mut best = f32::INFINITY;
            let mut best_q = p;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let k = (base.0 + dx, base.1 + dy, base.2 + dz);
                        if let Some(list) = grid.get(&k) {
                            for &ti in list {
                                let t = gt.tris[ti];
                                let a = gt.verts[t[0] as usize];
                                let b = gt.verts[t[1] as usize];
                                let c = gt.verts[t[2] as usize];
                                let q = closest_on_tri(p, a, b, c);
                                let d = (q[0] - p[0]).powi(2)
                                    + (q[1] - p[1]).powi(2)
                                    + (q[2] - p[2]).powi(2);
                                if d < best {
                                    best = d;
                                    best_q = q;
                                }
                            }
                        }
                    }
                }
            }
            if best.is_finite() {
                for k in 0..3 {
                    verts[i][k] = old[i][k] + lambda * (best_q[k] - old[i][k]);
                }
            }
        }
    }

    let mut out = String::from("# projected\n");
    for p in &verts {
        out.push_str(&format!("v {:.6} {:.6} {:.6}\n", p[0], p[1], p[2]));
    }
    for t in &rc.tris {
        out.push_str(&format!("f {} {} {}\n", t[0] + 1, t[1] + 1, t[2] + 1));
    }
    std::fs::write(&out_path, out).expect("写 OBJ");
    println!(
        "{{\"verts\":{},\"tris\":{},\"iters\":{},\"lambda\":{}}}",
        verts.len(),
        rc.tris.len(),
        iters,
        lambda
    );
}
