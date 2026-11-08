//! 训练高斯头并导出标准 3DGS PLY（给 `gauss2mesh` 吃）。
//!
//! 运行：`cargo run -q -p trit-3d --example gaussian_asset -- <out_dir>`

use trit_3d::gaussian::{GaussianHead, ShDegree, to_gaussians, train_gaussian_head, write_ply};
use trit_3d::{DenseEncoder, VoxelGrid};

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    std::fs::create_dir_all(&out_dir).expect("create out_dir");
    let g = VoxelGrid::cube(8, 2);
    let mut enc = DenseEncoder::new(8, 64, 32, 11);
    let mut head = GaussianHead::with_degree(enc.latent_dim(), 64, ShDegree::One, 11);
    let r = train_gaussian_head(&mut enc, &mut head, &g, 500, 0.05);
    let out = head.forward(&enc.encode(&g));
    let gs = to_gaussians(&out, ShDegree::One);
    let ply = std::path::Path::new(&out_dir).join("neural_gaussians.ply");
    write_ply(&gs, &ply).expect("write ply");
    println!(
        "{{\"ply\":\"{}\",\"n\":{},\"color_last\":{:.6}}}",
        ply.display(),
        gs.len(),
        r.last_color
    );
}
