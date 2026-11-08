//! 真实 OBJ → 表面体素编码 → 高斯预测 → PLY。
//!
//! 运行：`cargo run -q -p trit-3d --example real_asset -- <in.obj> <out_dir> [n]`

use trit_3d::DenseEncoder;
use trit_3d::gaussian::{
    GaussianHead, ShDegree, to_gaussians, train_gaussian_head_with_targets, write_ply,
};
use trit_3d::mesh::TriMesh;

fn main() {
    let obj = std::env::args().nth(1).expect("用法: real_asset <in.obj> <out_dir> [n]");
    let out_dir = std::env::args().nth(2).unwrap_or_else(|| ".".to_string());
    let n: usize = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "512".to_string())
        .parse()
        .expect("n 是整数");
    std::fs::create_dir_all(&out_dir).expect("create out_dir");

    let mut mesh = TriMesh::from_obj(std::path::Path::new(&obj)).expect("读 OBJ");
    mesh.normalize();
    let grid = mesh.voxelize_surface(16);
    let targets = mesh.surface_gaussians(n, ShDegree::One);

    let mut enc = DenseEncoder::new(16, 64, 32, 11);
    let mut head = GaussianHead::with_degree(enc.latent_dim(), n, ShDegree::One, 11);
    let r = train_gaussian_head_with_targets(&mut enc, &mut head, &grid, &targets, 500, 0.05);

    let out = head.forward(&enc.encode(&grid));
    let mut gs = to_gaussians(&out, ShDegree::One);
    for g in &mut gs {
        for k in 0..3 {
            g.scale[k] *= 1.8;
        }
    }
    let ply = std::path::Path::new(&out_dir).join("real_asset_gaussians.ply");
    write_ply(&gs, &ply).expect("写 PLY");
    println!(
        "{{\"obj\":\"{}\",\"verts\":{},\"tris\":{},\"gaussians\":{},\"mse_last\":{:.6},\"color_last\":{:.6}}}",
        obj,
        mesh.verts.len(),
        mesh.tris.len(),
        gs.len(),
        r.last_loss,
        r.last_color
    );
}
