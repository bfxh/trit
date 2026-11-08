//! 真实 mesh → **GT 表面高斯** → PLY（不经预测，用于分离表示/参数问题）。
//!
//! 运行：`cargo run -q -p trit-3d --example mesh_gt -- <out_dir> [n]`

use trit_3d::gaussian::{ShDegree, write_ply};
use trit_3d::mesh::TriMesh;

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let n: usize = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "2048".to_string())
        .parse()
        .expect("n 是整数");
    std::fs::create_dir_all(&out_dir).expect("create out_dir");
    let mut mesh = TriMesh::torus(0.7, 0.3, 24, 12);
    mesh.normalize();
    let gs = mesh.surface_gaussians(n, ShDegree::One);
    let ply = std::path::Path::new(&out_dir).join("torus_gt.ply");
    write_ply(&gs, &ply).expect("write ply");
    println!(
        "{{\"ply\":\"{}\",\"n\":{},\"scale\":{:.6}}}",
        ply.display(),
        gs.len(),
        gs[0].scale[0]
    );
}
