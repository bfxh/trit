//! 双面壳重建输入：把单层表面高斯沿法线 ±t/2 复制成内外两层 PLY。
//!
//! 运行：`cargo run --release -q -p trit-3d --example double_shell -- <in.obj> <out_dir> [n] [t]`

use trit_3d::gaussian::{Gaussian, ShDegree, write_ply};
use trit_3d::mesh::TriMesh;

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

fn quat_from_z(n: [f32; 3]) -> [f32; 4] {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
    let z = [n[0] / l, n[1] / l, n[2] / l];
    let d = z[2].clamp(-1.0, 1.0);
    if d < -0.999_999 {
        return [0.0, 1.0, 0.0, 0.0];
    }
    let axis = [-z[1], z[0], 0.0];
    let s = ((1.0 + d) * 2.0).sqrt();
    [s * 0.5, axis[0] / s, axis[1] / s, axis[2] / s]
}

fn main() {
    let obj = std::env::args().nth(1).expect("用法: double_shell <in.obj> <out_dir> [n] [t]");
    let out_dir = std::env::args().nth(2).unwrap_or_else(|| ".".to_string());
    let n: usize = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "4096".to_string())
        .parse()
        .expect("n");
    let t: f32 = std::env::args()
        .nth(4)
        .unwrap_or_else(|| "0.03".to_string())
        .parse()
        .expect("t");
    std::fs::create_dir_all(&out_dir).expect("out_dir");

    let mut mesh = TriMesh::from_obj(std::path::Path::new(&obj)).expect("读 OBJ");
    mesh.normalize();
    let gs = mesh.surface_gaussians(n, ShDegree::One);

    let mut outer: Vec<Gaussian> = Vec::with_capacity(gs.len());
    let mut inner: Vec<Gaussian> = Vec::with_capacity(gs.len());
    for g in &gs {
        let nrm = axis_z(g.rotation);
        let mut a = g.clone();
        for k in 0..3 {
            a.pos[k] += nrm[k] * t * 0.5;
        }
        a.rotation = quat_from_z(nrm);
        outer.push(a);

        let mut b = g.clone();
        for k in 0..3 {
            b.pos[k] -= nrm[k] * t * 0.5;
        }
        b.rotation = quat_from_z([-nrm[0], -nrm[1], -nrm[2]]);
        inner.push(b);
    }

    let out_ply = std::path::Path::new(&out_dir).join("shell_out.ply");
    let in_ply = std::path::Path::new(&out_dir).join("shell_in.ply");
    write_ply(&outer, &out_ply).expect("写外 PLY");
    write_ply(&inner, &in_ply).expect("写内 PLY");
    println!(
        "{{\"outer\":\"{}\",\"inner\":\"{}\",\"gaussians\":{},\"t\":{}}}",
        out_ply.display(),
        in_ply.display(),
        gs.len(),
        t
    );
}
