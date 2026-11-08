//! 真实 mesh（torus）→ 体素编码 → 高斯预测 → PLY。
//!
//! 运行：`cargo run -q -p trit-3d --example mesh_asset -- <out_dir> [n]`

use trit_3d::DenseEncoder;
use trit_3d::gaussian::{
    GaussianHead, ShDegree, to_gaussians, train_gaussian_head_with_targets, write_ply,
};
use trit_3d::mesh::{TriMesh, chamfer_points};

fn axis_z(q: [f32; 4]) -> [f32; 3] {
    let [w, x, y, z] = q;
    let v = [
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    ];
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let n: usize = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "512".to_string())
        .parse()
        .expect("n 是整数");
    std::fs::create_dir_all(&out_dir).expect("create out_dir");

    let mut mesh = TriMesh::torus(0.7, 0.3, 24, 12);
    mesh.normalize();
    let grid = mesh.voxelize(16);
    let targets = mesh.surface_gaussians(n, ShDegree::One);

    let mut enc = DenseEncoder::new(16, 128, 64, 11);
    let mut head = GaussianHead::with_degree(enc.latent_dim(), n, ShDegree::One, 11);
    let r = train_gaussian_head_with_targets(&mut enc, &mut head, &grid, &targets, 500, 0.05);

    let out = head.forward(&enc.encode(&grid));
    let mut gs = to_gaussians(&out, ShDegree::One);
    for g in &mut gs {
        for k in 0..3 {
            g.scale[k] *= 1.8;
        }
    }
    let pred: Vec<[f32; 3]> = gs.iter().map(|g| g.pos).collect();
    let gt = mesh.surface_points(1024);
    let chamfer = chamfer_points(&pred, &gt);

    let mut dot = 0.0f32;
    let mut scale_err = 0.0f32;
    for (p, t) in gs.iter().zip(&targets) {
        let np = axis_z(p.rotation);
        let nt = axis_z(t.rotation);
        dot += (np[0] * nt[0] + np[1] * nt[1] + np[2] * nt[2]).abs();
        for k in 0..3 {
            scale_err += ((p.scale[k] - t.scale[k]) / t.scale[k].max(1e-6)).abs();
        }
    }
    dot /= gs.len() as f32;
    scale_err /= (gs.len() * 3) as f32;

    let ply = std::path::Path::new(&out_dir).join("torus_gaussians.ply");
    write_ply(&gs, &ply).expect("write ply");
    println!(
        "{{\"ply\":\"{}\",\"verts\":{},\"tris\":{},\"gaussians\":{},\"mse_last\":{:.6},\"color_last\":{:.6},\"chamfer2\":{:.6},\"normal_dot\":{:.4},\"scale_rel_err\":{:.4}}}",
        ply.display(),
        mesh.verts.len(),
        mesh.tris.len(),
        gs.len(),
        r.last_loss,
        r.last_color,
        chamfer,
        dot,
        scale_err
    );
}
