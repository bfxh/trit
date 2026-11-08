//! 真实 OBJ → GT 表面高斯 → PLY（不经预测，用于分离“表示极限”与“预测误差”）。
//!
//! 运行：`cargo run --release -q -p trit-3d --example real_gt -- <in.obj> <out_dir> [n]`

use trit_3d::gaussian::{ShDegree, write_ply};
use trit_3d::mesh::TriMesh;

fn main() {
    let obj = std::env::args().nth(1).expect("用法: real_gt <in.obj> <out_dir> [n]");
    let out_dir = std::env::args().nth(2).unwrap_or_else(|| ".".to_string());
    let n: usize = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "4096".to_string())
        .parse()
        .expect("n");
    std::fs::create_dir_all(&out_dir).expect("out_dir");
    let mut mesh = TriMesh::from_obj(std::path::Path::new(&obj)).expect("读 OBJ");
    mesh.normalize();
    let gs = mesh.surface_gaussians(n, ShDegree::One);
    let ply = std::path::Path::new(&out_dir).join("real_gt_gaussians.ply");
    write_ply(&gs, &ply).expect("写 PLY");
    println!(
        "{{\"ply\":\"{}\",\"gaussians\":{},\"scale\":{:.6}}}",
        ply.display(),
        gs.len(),
        gs[0].scale[0]
    );
}
