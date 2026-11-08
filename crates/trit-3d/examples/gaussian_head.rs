//! 8³ 立方体 → 64 个完整 3DGS 高斯（1 阶 SH）的读数。
//!
//! 运行：`cargo run -p trit-3d --example gaussian_head`

use trit_3d::gaussian::{GaussianHead, ShDegree, to_gaussians, train_gaussian_head};
use trit_3d::{DenseEncoder, VoxelGrid};

fn main() {
    let g = VoxelGrid::cube(8, 2);
    let mut enc = DenseEncoder::new(8, 64, 32, 11);
    let mut head = GaussianHead::with_degree(enc.latent_dim(), 64, ShDegree::One, 11);
    let r = train_gaussian_head(&mut enc, &mut head, &g, 500, 0.05);
    let out = head.forward(&enc.encode(&g));
    let gs = to_gaussians(&out, ShDegree::One);
    let high: f32 = gs
        .iter()
        .map(|g| g.sh[3..].iter().map(|v| v.abs()).sum::<f32>())
        .sum();
    println!(
        "{{\"n\":{},\"params_per_gaussian\":{},\"sh_degree\":\"{:?}\",\"mse_first\":{:.6},\"mse_last\":{:.6},\"color_first\":{:.6},\"color_last\":{:.6},\"chamfer2_first\":{:.6},\"chamfer2_last\":{:.6},\"high_order_sh_abs_sum\":{:.4}}}",
        r.n,
        r.params,
        r.degree,
        r.first_loss,
        r.last_loss,
        r.first_color,
        r.last_color,
        r.first_chamfer,
        r.last_chamfer,
        high
    );
}
