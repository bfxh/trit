//! `export-asset`：把训练好的 N2 占据模型在**单个场景**上导成多档 LOD 网格资产。
//!
//! 不读 `objects.json`（真实场景没有）：栅格由多视角点云建（与自监督训练同源），
//! 模型逐体素推理 → 清边界/填实/修对角 → Surface Nets → PLY；每档一个文件 + 清单。
//!
//! ```text
//! parts-3d export-asset --model <occ1> --scene <dir> --frames 0,1,...,7 \
//!   --out <dir> [--cells 0.01,0.02,0.04]
//! ```

use std::path::PathBuf;

use trit_parts::data::{read_npy_i32, read_rgb};
use trit_parts::geo::{Camera, read_npy_f32};
use trit_parts::mesh::{Grid, boundary_edges, non_manifold_edges, surface_nets, write_ply_rgb};
use trit_parts::occ::Mlp;

use super::cli::{arg, fail, frame_path, usage};
use super::occ::model_solid;
use super::splat::view_points;

struct Level {
    cell: f32,
    name: String,
    bytes: u64,
    sha256: String,
    verts: usize,
    tris: usize,
    boundary: usize,
    nonmanifold: usize,
    /// 没有任何视角看到的顶点数（标灰）。
    unseen: usize,
    tex: Option<Tex>,
}

/// 一档的 UV 图集资产（OBJ + MTL + PNG）。
struct Tex {
    obj: String,
    obj_bytes: u64,
    obj_sha: String,
    png: String,
    png_bytes: u64,
    png_sha: String,
    atlas: usize,
    charts: usize,
    texels: usize,
    unseen: usize,
}

pub(crate) fn export_asset_cmd(argv: &[String]) {
    let model_path = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let cells: Vec<f32> = arg(argv, "--cells")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0.01, 0.02, 0.04]);
    if cells.iter().any(|c| !(*c > 0.0)) {
        fail("--cells 必须是正数");
    }
    // 纹素级材质：UV 图集 + OBJ/MTL + PNG（默认关，清单里记 texture 开关）
    let texture = argv.iter().any(|a| a == "--texture");
    let atlas_px: usize = arg(argv, "--atlas")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    let texels_per_m: f32 = arg(argv, "--texels-per-m")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let normal_deg: f32 = arg(argv, "--atlas-deg")
        .and_then(|v| v.parse().ok())
        .unwrap_or(45.0);
    if texture && atlas_px < 64 {
        fail("--atlas 至少 64 像素");
    }
    let mlp = Mlp::load(&model_path).unwrap_or_else(|e| fail(&e));

    let load = |f: usize| -> trit_parts::fuse::View {
        let (ids, shape) =
            read_npy_i32(&frame_path(&scene, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
        let (d, _) =
            read_npy_f32(&frame_path(&scene, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
        trit_parts::fuse::View {
            ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
            depth: d,
            w: shape[1],
            h: shape[0],
            cam: Camera::load(&frame_path(&scene, "meta", f, "json")).unwrap_or_else(|e| fail(&e)),
        }
    };
    let views: Vec<trit_parts::fuse::View> = frames.iter().map(|&f| load(f)).collect();
    let rgbs: Vec<Vec<f32>> = frames
        .iter()
        .map(|&f| {
            read_rgb(&frame_path(&scene, "rgb", f, "png"))
                .map(|(_, _, p)| p)
                .unwrap_or_else(|e| fail(&e))
        })
        .collect();
    let pts = view_points(&views);
    std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));

    let mut levels = Vec::new();
    for &cell in &cells {
        let g = match Grid::from_points(&pts, cell, 3) {
            Some(g) => g,
            None => fail(&format!("cell {cell} 建不出栅格")),
        };
        let occ = model_solid(&mlp, &views, &g);
        let mut solved = g.clone();
        solved.occ = occ;
        let m = surface_nets(&solved);
        let name = format!("lod-{}cm.ply", (cell * 100.0).round() as u32);
        let p = out.join(&name);
        // 顶点色：投影到每个视角，深度一致才采样，跨视角平均；无视角看到 ⇒ 标灰。
        let colors: Vec<Option<[u8; 3]>> = m
            .verts
            .iter()
            .map(|v| {
                trit_parts::splat::bake_view_color(&views, &rgbs, *v).map(|c| {
                    [
                        (c[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                        (c[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                        (c[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                    ]
                })
            })
            .collect();
        let unseen = colors.iter().filter(|c| c.is_none()).count();
        write_ply_rgb(&p, &m, &colors).unwrap_or_else(|e| fail(&e));
        let tex = if texture {
            let stem = format!("lod-{}cm", (cell * 100.0).round() as u32);
            let png_name = format!("{stem}.png");
            let obj_name = format!("{stem}.obj");
            let (mut texels, mut unseen_texels) = (0usize, 0usize);
            let at = trit_parts::atlas::bake_atlas_with(
                &m,
                atlas_px,
                texels_per_m,
                normal_deg,
                [0.5, 0.5, 0.5],
                |_fi, pos| {
                    texels += 1;
                    let c = trit_parts::splat::bake_view_color(&views, &rgbs, pos);
                    if c.is_none() {
                        unseen_texels += 1;
                    }
                    c
                },
            )
            .unwrap_or_else(|e| fail(&e));
            let pp = out.join(&png_name);
            trit_parts::data::write_rgb(&pp, at.w, at.h, &at.img).unwrap_or_else(|e| fail(&e));
            let op = out.join(&obj_name);
            trit_parts::uv::write_obj(&op, &m, &at.uv, &png_name).unwrap_or_else(|e| fail(&e));
            println!(
                "  图集 {}²：chart {}，纹素 {}（无观测 {}）→ {obj_name} + {png_name}",
                at.w,
                at.charts.len(),
                texels,
                unseen_texels
            );
            Some(Tex {
                obj_bytes: std::fs::metadata(&op).map(|x| x.len()).unwrap_or(0),
                obj_sha: trit_core::fingerprint::file_sha256(&op).unwrap_or_default(),
                png_bytes: std::fs::metadata(&pp).map(|x| x.len()).unwrap_or(0),
                png_sha: trit_core::fingerprint::file_sha256(&pp).unwrap_or_default(),
                obj: obj_name,
                png: png_name,
                atlas: at.w,
                charts: at.charts.len(),
                texels,
                unseen: unseen_texels,
            })
        } else {
            None
        };
        let bytes = std::fs::metadata(&p).map(|x| x.len()).unwrap_or(0);
        let sha = trit_core::fingerprint::file_sha256(&p).unwrap_or_default();
        let boundary = boundary_edges(&m);
        let nonmanifold = non_manifold_edges(&m);
        println!(
            "cell {cell} m: 顶点 {} 面 {} 边界边 {} 非流形 {} 无观测顶点 {} → {name}",
            m.verts.len(),
            m.tris.len(),
            boundary,
            nonmanifold,
            unseen
        );
        levels.push(Level {
            cell,
            name,
            bytes,
            sha256: sha,
            verts: m.verts.len(),
            tris: m.tris.len(),
            boundary,
            nonmanifold,
            unseen,
            tex,
        });
    }

    let fp =
        trit_parts::scenefp::fingerprint_scene_frames(&scene, &frames).unwrap_or_else(|e| fail(&e));
    let (files, data_bytes, data_hash) = (fp.files, fp.bytes, fp.hash);
    let model_sha = trit_core::fingerprint::file_sha256(&model_path).unwrap_or_default();
    let model_bytes = std::fs::metadata(&model_path).map(|m| m.len()).unwrap_or(0);
    let commit = trit_parts::partsrun::git_out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let dirty = !trit_parts::partsrun::git_out(&["status", "--porcelain"])
        .trim()
        .is_empty();
    let argv_json: Vec<String> = std::env::args()
        .skip(1)
        .map(|a| trit_parts::partsrun::json_str(&a))
        .collect();
    let frames_json: Vec<String> = frames.iter().map(|f| f.to_string()).collect();
    let cells_json: Vec<String> = cells.iter().map(|c| format!("{c}")).collect();
    let levels_json: Vec<String> = levels
        .iter()
        .map(|l| {
            format!(
                "{{\"cell\": {}, \"path\": {}, \"sha256\": \"{}\", \"bytes\": {}, \"verts\": {}, \"tris\": {}, \"boundary_edges\": {}, \"nonmanifold_edges\": {}, \"unseen_vertices\": {}}}",
                l.cell,
                trit_parts::partsrun::json_str(&l.name),
                l.sha256,
                l.bytes,
                l.verts,
                l.tris,
                l.boundary,
                l.nonmanifold,
                l.unseen
            )
        })
        .collect();
    let textures_json: Vec<String> = levels
        .iter()
        .filter_map(|l| l.tex.as_ref().map(|t| (l.cell, t)))
        .map(|(cell, t)| {
            format!(
                "{{\"cell\": {cell}, \"path\": {}, \"sha256\": \"{}\", \"bytes\": {}, \"png_path\": {}, \"png_sha256\": \"{}\", \"png_bytes\": {}, \"atlas\": {}, \"charts\": {}, \"texels\": {}, \"unseen_texels\": {}}}",
                trit_parts::partsrun::json_str(&t.obj),
                t.obj_sha,
                t.obj_bytes,
                trit_parts::partsrun::json_str(&t.png),
                t.png_sha,
                t.png_bytes,
                t.atlas,
                t.charts,
                t.texels,
                t.unseen
            )
        })
        .collect();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-asset\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"frames\": [{}], \"cells\": [{}], \"bake_colors\": true, \"texture\": {texture}, \"atlas\": {atlas_px}, \"texels_per_m\": {texels_per_m}, \"normal_deg\": {normal_deg}}},\n  \"data\": {{\"files\": {files}, \"bytes\": {data_bytes}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{data_hash}\", \"scene\": {}}},\n  \"model\": {{\"path\": {}, \"sha256\": \"{model_sha}\", \"bytes\": {model_bytes}}},\n  \"levels\": [{}],\n  \"textures\": [{}]\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        frames_json.join(", "),
        cells_json.join(", "),
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&scene)),
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&model_path)),
        levels_json.join(", "),
        textures_json.join(", ")
    );
    let mp = out.join("asset.manifest.json");
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!("清单 {}", mp.display());
}
