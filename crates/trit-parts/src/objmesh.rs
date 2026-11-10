//! 对象级网格：从多视角融合按部件取**视觉外壳**（`Fused::visual_hull`），
//! Surface Nets 出网格，再把视图颜色烘成 UV 图集——每对象一份 OBJ/MTL/PNG。
//!
//! 视觉外壳用「投到任一视角背景就剪」+ 填实，底面即使没被看到也被剪影封住；
//! 因此每对象是闭合体（边界边 0），但凹处/遮挡区会保守保留（剪影锥误差）。

use std::collections::HashMap;

use crate::atlas::{Atlas, bake_atlas_with};
use crate::fuse::{FuseCfg, View, fuse};
use crate::mesh::{Mesh, boundary_edges, non_manifold_edges};
use crate::splat::bake_view_color;
use crate::splat::{Item, Placement, transform_point};

/// 一个对象的网格 + 图集与统计。
pub struct Built {
    pub id: u32,
    pub mesh: Mesh,
    pub atlas: Atlas,
    pub boundary: usize,
    pub nonmanifold: usize,
    pub texels: usize,
    pub unseen: usize,
}

/// 按 `ids` 顺序构建（只返回有面的对象）。融合失败/图集失败都返回错误，不静默跳。
pub fn build_objects(
    views: &[View],
    rgbs: &[Vec<f32>],
    cell: f32,
    atlas_px: usize,
    normal_deg: f32,
    ids: &[u32],
) -> Result<Vec<Built>, String> {
    let fused = fuse(
        views,
        &FuseCfg {
            cell,
            margin: 2,
            min_votes: 1.0,
            seal: 1,
        },
    )
    .ok_or("融合失败：没有有效深度样本")?;
    let mut hulls: HashMap<u32, Mesh> = fused
        .hull_meshes(views)
        .into_iter()
        .map(|h| (h.id, h.mesh))
        .collect();
    let mut out = Vec::new();
    for &id in ids {
        let Some(mesh) = hulls.remove(&id) else {
            continue;
        };
        if mesh.tris.is_empty() {
            continue;
        }
        let (mut texels, mut unseen) = (0usize, 0usize);
        let atlas = bake_atlas_with(&mesh, atlas_px, 0.0, normal_deg, [0.5; 3], |_fi, p| {
            texels += 1;
            let c = bake_view_color(views, rgbs, p);
            if c.is_none() {
                unseen += 1;
            }
            c
        })?;
        out.push(Built {
            id,
            boundary: boundary_edges(&mesh),
            nonmanifold: non_manifold_edges(&mesh),
            mesh,
            atlas,
            texels,
            unseen,
        });
    }
    Ok(out)
}

/// 场景里的一件：(id, 网格, 每三角形三角的 UV)。
pub type ScenePart = (u32, Mesh, Vec<[[f32; 2]; 3]>);

/// 合成场景 OBJ/MTL：顶点按布局变换，`g` 分组保留对象边界。
pub fn write_scene(
    dir: &std::path::Path,
    built: &[ScenePart],
    items: &HashMap<u32, Item>,
    places: &HashMap<u32, Placement>,
) -> Result<(), String> {
    let mut s = String::from("mtllib scene.mtl\n");
    let mut mtl = String::new();
    let (mut vbase, mut tbase) = (0usize, 0usize);
    for (id, m, uv) in built {
        let it = items.get(id).ok_or_else(|| format!("对象 {id} 缺 bbox"))?;
        let pl = places.get(id).ok_or_else(|| format!("对象 {id} 缺布局"))?;
        s.push_str(&format!("o obj-{id}\ng obj-{id}\n"));
        for v in &m.verts {
            let q = transform_point(*v, it, pl);
            s.push_str(&format!("v {} {} {}\n", q[0], q[1], q[2]));
        }
        for tri in uv {
            for p in tri {
                s.push_str(&format!("vt {:.6} {:.6}\n", p[0], p[1]));
            }
        }
        s.push_str(&format!("usemtl mat-{id}\n"));
        for (i, t) in m.tris.iter().enumerate() {
            let b = tbase + 3 * i;
            s.push_str(&format!(
                "f {}/{} {}/{} {}/{}\n",
                vbase + t[0] as usize + 1,
                b + 1,
                vbase + t[1] as usize + 1,
                b + 2,
                vbase + t[2] as usize + 1,
                b + 3
            ));
        }
        vbase += m.verts.len();
        tbase += 3 * m.tris.len();
        mtl.push_str(&format!(
            "newmtl mat-{id}\nKa 1.000 1.000 1.000\nKd 1.000 1.000 1.000\nKs 0.000 0.000 0.000\nillum 1\nmap_Kd obj-{id}.png\n\n"
        ));
    }
    std::fs::write(dir.join("scene.obj"), s).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("scene.mtl"), mtl).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::Camera;

    fn cam() -> Camera {
        Camera {
            k: [[100.0, 0.0, 50.0], [0.0, 100.0, 50.0], [0.0, 0.0, 1.0]],
            t_cw: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    #[test]
    fn scene_obj_applies_layout_transforms() {
        let m = Mesh {
            verts: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            tris: vec![[0, 1, 2]],
        };
        let uv = vec![[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]];
        let built = vec![(1u32, m, uv)];
        let items = HashMap::from([(
            1u32,
            Item {
                id: 1,
                bbox_min: [0.0, 0.0, 0.0],
                bbox_max: [1.0, 1.0, 0.0],
            },
        )]);
        let places = HashMap::from([(
            1u32,
            Placement {
                id: 1,
                yaw: 90,
                translate: [0.0, 0.0, 0.0],
                bbox_min: [0.0, 0.0, 0.0],
                bbox_max: [0.0, 1.0, 1.0],
            },
        )]);
        let dir = std::env::temp_dir().join(format!("trit_scene_obj_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_scene(&dir, &built, &items, &places).unwrap();
        let obj = std::fs::read_to_string(dir.join("scene.obj")).unwrap();
        assert!(obj.contains("g obj-1"));
        assert!(obj.contains("usemtl mat-1"));
        // (1,0,0) 绕 bbox 中心 (0.5,0.5) 转 90° → (0.5, 0, -0.5)
        assert!(obj.contains("v 0.5 0 -0.5"), "{obj}");
        let mtl = std::fs::read_to_string(dir.join("scene.mtl")).unwrap();
        assert!(mtl.contains("newmtl mat-1"));
        assert!(mtl.contains("map_Kd obj-1.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_one_closed_mesh_per_id() {
        // 100×100 画面里左半 id=1、右半 id=2，深度 2 m
        let (w, h) = (100usize, 100usize);
        let mut ids = vec![1u32; w * h];
        for y in 0..h {
            for x in 50..w {
                ids[y * w + x] = 2;
            }
        }
        let view = View {
            ids,
            depth: vec![2.0; w * h],
            w,
            h,
            cam: cam(),
        };
        let rgb: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                if (i % w) < 50 {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 1.0, 0.0]
                }
            })
            .collect();
        let built = build_objects(
            std::slice::from_ref(&view),
            std::slice::from_ref(&rgb),
            0.05,
            64,
            45.0,
            &[1, 2],
        )
        .unwrap();
        assert_eq!(built.len(), 2);
        for b in &built {
            assert!(!b.mesh.tris.is_empty(), "对象 {} 应有面", b.id);
            assert_eq!(b.boundary, 0, "视觉外壳应闭合（边界边 0）");
            assert!(b.texels > 0, "图集应有纹素");
            assert!(b.unseen <= b.texels);
        }
        assert_eq!(built[0].id, 1);
        assert_eq!(built[1].id, 2);
        // 颜色只来自各自半侧：id=1 应偏红、id=2 偏绿
        let mean = |b: &Built| {
            let n = (b.atlas.w * b.atlas.h) as f32;
            let mut acc = [0.0f32; 3];
            let mut k = 0usize;
            for p in b.atlas.img.chunks(3) {
                if p[0] != 0.5 || p[1] != 0.5 || p[2] != 0.5 {
                    for c in 0..3 {
                        acc[c] += p[c];
                    }
                    k += 1;
                }
            }
            let d = k.max(1) as f32;
            let _ = n;
            [acc[0] / d, acc[1] / d, acc[2] / d]
        };
        let m1 = mean(&built[0]);
        let m2 = mean(&built[1]);
        assert!(m1[0] > m1[1], "id=1 应偏红：{m1:?}");
        assert!(m2[1] > m2[0], "id=2 应偏绿：{m2:?}");
    }
}
