//! 重建 mesh 后处理：Laplacian 平滑（减少 surfel 褶皱/面积），用于薄壳验收实验。
//!
//! 运行：`cargo run --release -q -p trit-3d --example smooth_mesh -- <in.obj> <out.obj> [iters] [lambda]`

use trit_3d::mesh::TriMesh;

fn main() {
    let input = std::env::args().nth(1).expect("用法: smooth_mesh <in.obj> <out.obj> [iters] [lambda]");
    let output = std::env::args().nth(2).expect("用法: smooth_mesh <in.obj> <out.obj> [iters] [lambda]");
    let iters: usize = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "1".to_string())
        .parse()
        .expect("iters");
    let lambda: f32 = std::env::args()
        .nth(4)
        .unwrap_or_else(|| "0.5".to_string())
        .parse()
        .expect("lambda");

    let mesh = TriMesh::from_obj(std::path::Path::new(&input)).expect("读 OBJ");
    let mut nbrs: Vec<Vec<u32>> = vec![Vec::new(); mesh.verts.len()];
    for t in &mesh.tris {
        for k in 0..3 {
            let a = t[k];
            let b = t[(k + 1) % 3];
            nbrs[a as usize].push(b);
            nbrs[b as usize].push(a);
        }
    }
    let mut v = mesh.verts.clone();
    for _ in 0..iters {
        let old = v.clone();
        for i in 0..v.len() {
            let ns = &nbrs[i];
            if ns.is_empty() {
                continue;
            }
            let mut sum = [0.0f32; 3];
            for &j in ns {
                for k in 0..3 {
                    sum[k] += old[j as usize][k];
                }
            }
            let inv = 1.0 / ns.len() as f32;
            for k in 0..3 {
                let avg = sum[k] * inv;
                v[i][k] = old[i][k] + lambda * (avg - old[i][k]);
            }
        }
    }

    let mut out = String::from("# smoothed\n");
    for p in &v {
        out.push_str(&format!("v {:.6} {:.6} {:.6}\n", p[0], p[1], p[2]));
    }
    for t in &mesh.tris {
        out.push_str(&format!("f {} {} {}\n", t[0] + 1, t[1] + 1, t[2] + 1));
    }
    std::fs::write(&output, out).expect("写 OBJ");
    println!(
        "{{\"verts\":{},\"tris\":{},\"iters\":{},\"lambda\":{}}}",
        v.len(),
        mesh.tris.len(),
        iters,
        lambda
    );
}
