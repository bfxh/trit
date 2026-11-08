//! 生成密集面元 PLY：读 OBJ → 表面采样 n 个带法线的面元 → 写 PLY。
//!
//! 用途：给三态符号场实验提供「面元密到能逐体素判定归属」的输入
//! （CHG-2026-11-09-Z 的判决认为这是三态能否兑现表达力的前提）。
//!
//! 运行：`cargo run --release -q -p trit-3d --example dense_surfel -- <in.obj> <out.ply> [n] [thin]`

use trit_3d::gaussian::{ShDegree, write_ply};
use trit_3d::mesh::TriMesh;

fn main() {
    let obj = std::env::args()
        .nth(1)
        .expect("用法: dense_surfel <in.obj> <out.ply> [n] [thin]");
    let out = std::env::args()
        .nth(2)
        .expect("用法: dense_surfel <in.obj> <out.ply> [n] [thin]");
    let n: usize = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "100000".to_string())
        .parse()
        .expect("n 是整数");
    let thin: f32 = std::env::args()
        .nth(4)
        .unwrap_or_else(|| "0.05".to_string())
        .parse()
        .expect("thin 是数值");

    let mesh = TriMesh::from_obj(std::path::Path::new(&obj)).expect("读 OBJ");
    let gs = mesh.surface_gaussians_thin(n, ShDegree::One, thin);
    write_ply(&gs, std::path::Path::new(&out)).expect("写 PLY");
    println!(
        "{{\"obj\":\"{}\",\"out\":\"{}\",\"surface_tris\":{},\"gaussians\":{},\"thin\":{}}}",
        obj,
        out,
        mesh.tris.len(),
        gs.len(),
        thin
    );
}
