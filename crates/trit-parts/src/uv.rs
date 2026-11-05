//! 盒式（triplanar 静态版）投影 UV：把贴图真正贴到网格上。
//!
//! 每个三角形按面法线的主轴投影到对应平面，UV 以 `tex_world`（一张贴图覆盖的世界尺寸）归一化，
//! 超出 `[0,1]` 的部分靠渲染器的 wrap repeat 平铺。输出 OBJ + MTL（`mesh-tex --tex` 用）。
//!
//! 局限：这是**投影式** UV，不是展开（unwrap）——接缝处会有连续性跳变，且不保证无重叠；
//! 对重建出来的稠密表面（体素/表面网）够用，对需要精确 UV 的美术资产不够。

use crate::mesh::Mesh;
use std::path::Path;

/// 每个三角形三个角的 `(u, v)`；`uv.len() == 3 · tris.len()` 顺序为 `(三角, 角)`。
pub fn box_uvs(m: &Mesh, tex_world: f32) -> Vec<[[f32; 2]; 3]> {
    let s = if tex_world.abs() < 1e-6 {
        1.0
    } else {
        tex_world
    };
    let mut out = Vec::with_capacity(m.tris.len());
    for t in &m.tris {
        let a = m.verts[t[0] as usize];
        let b = m.verts[t[1] as usize];
        let c = m.verts[t[2] as usize];
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let ax = [n[0].abs(), n[1].abs(), n[2].abs()];
        let axis = if ax[0] >= ax[1] && ax[0] >= ax[2] {
            0
        } else if ax[1] >= ax[2] {
            1
        } else {
            2
        };
        let sign = if n[axis] < 0.0 { -1.0 } else { 1.0 };
        let mut uv = [[0.0f32; 2]; 3];
        for (k, v) in [a, b, c].iter().enumerate() {
            let (mut u, vv) = match axis {
                0 => (v[2], v[1]),
                1 => (v[0], v[2]),
                _ => (v[0], v[1]),
            };
            // 背面翻转 u，避免同一块贴图在两侧互为镜像
            if axis != 1 {
                u *= sign;
            }
            uv[k] = [u / s, vv / s];
        }
        out.push(uv);
    }
    out
}

/// 写 OBJ + MTL（贴图文件名写进 `map_Kd`）。`uv` 来自 [`box_uvs`]。
pub fn write_obj(
    path: &Path,
    m: &Mesh,
    uv: &[[[f32; 2]; 3]],
    tex_name: &str,
) -> Result<(), String> {
    use std::fmt::Write as _;
    if uv.len() != m.tris.len() {
        return Err(format!("UV 数 {} != 三角形数 {}", uv.len(), m.tris.len()));
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh");
    let mut s = String::new();
    writeln!(s, "mtllib {stem}.mtl").map_err(|e| e.to_string())?;
    for v in &m.verts {
        writeln!(s, "v {} {} {}", v[0], v[1], v[2]).map_err(|e| e.to_string())?;
    }
    for tri in uv {
        for p in tri {
            writeln!(s, "vt {:.6} {:.6}", p[0], p[1]).map_err(|e| e.to_string())?;
        }
    }
    writeln!(s, "usemtl tex").map_err(|e| e.to_string())?;
    for (i, t) in m.tris.iter().enumerate() {
        let b = 3 * i + 1;
        writeln!(
            s,
            "f {}/{} {}/{} {}/{}",
            t[0] as usize + 1,
            b,
            t[1] as usize + 1,
            b + 1,
            t[2] as usize + 1,
            b + 2
        )
        .map_err(|e| e.to_string())?;
    }
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))?;
    let mtl = format!(
        "newmtl tex\nKa 1.000 1.000 1.000\nKd 1.000 1.000 1.000\nKs 0.000 0.000 0.000\n\
         illum 1\nmap_Kd {tex_name}\n"
    );
    let mp = path.with_file_name(format!("{stem}.mtl"));
    std::fs::write(&mp, mtl).map_err(|e| format!("{}: {e}", mp.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane() -> Mesh {
        Mesh {
            verts: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            tris: vec![[0, 1, 2], [0, 2, 3]],
        }
    }

    #[test]
    fn plane_facing_z_projects_xy_unchanged() {
        let m = plane();
        let uv = box_uvs(&m, 2.0);
        assert_eq!(uv.len(), 2);
        // 法线 +Z ⇒ u = x / tex_world, v = y / tex_world
        assert!((uv[0][0][0] - 0.0).abs() < 1e-6);
        assert!((uv[0][1][0] - 1.0).abs() < 1e-6, "x=2, tex_world=2 ⇒ u=1");
        assert!((uv[0][2][1] - 0.5).abs() < 1e-6, "y=1 ⇒ v=0.5");
    }

    #[test]
    fn sphere_uvs_are_finite_and_complete() {
        let (nx, ny) = (16usize, 8usize);
        let mut verts = Vec::new();
        for j in 0..=ny {
            let phi = std::f32::consts::PI * j as f32 / ny as f32;
            for i in 0..nx {
                let th = std::f32::consts::TAU * i as f32 / nx as f32;
                verts.push([phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin()]);
            }
        }
        let mut tris = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                let a = (j * nx + i) as u32;
                let b = (j * nx + (i + 1) % nx) as u32;
                let c = ((j + 1) * nx + i) as u32;
                let d = ((j + 1) * nx + (i + 1) % nx) as u32;
                tris.push([a, b, c]);
                tris.push([b, d, c]);
            }
        }
        let m = Mesh { verts, tris };
        let uv = box_uvs(&m, 1.0);
        assert_eq!(uv.len(), m.tris.len());
        assert!(uv.iter().flatten().flatten().all(|v| v.is_finite()));
        // 投影 UV 会超出 [0,1]（靠 wrap 平铺），但不能是常数
        let span = uv.iter().flatten().map(|p| p[0]).fold(f32::MIN, f32::max)
            - uv.iter().flatten().map(|p| p[0]).fold(f32::MAX, f32::min);
        assert!(span > 1.0, "UV 应有实际跨度，实际 {span}");
    }

    #[test]
    fn obj_lists_one_vt_per_corner_and_matching_faces() {
        let m = plane();
        let uv = box_uvs(&m, 2.0);
        let p = std::env::temp_dir().join("trit_parts_uv_test.obj");
        write_obj(&p, &m, &uv, "tex.png").unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(s.lines().filter(|l| l.starts_with("vt ")).count(), 6);
        assert_eq!(s.lines().filter(|l| l.starts_with("f ")).count(), 2);
        assert!(s.contains("mtllib trit_parts_uv_test.mtl"));
        let mtl = std::fs::read_to_string(p.with_extension("mtl")).unwrap();
        assert!(mtl.contains("map_Kd tex.png"));
        std::fs::remove_file(&p).ok();
        std::fs::remove_file(p.with_extension("mtl")).ok();
    }
}
