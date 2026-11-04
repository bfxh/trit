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

use trit_parts::data::read_npy_i32;
use trit_parts::geo::{Camera, read_npy_f32};
use trit_parts::mesh::{Grid, boundary_edges, non_manifold_edges, surface_nets, write_ply};
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
}

/// 场景输入指纹（模型消费的就是 seg/depth/meta 三类）。
fn scene_fingerprint(
    scene: &std::path::Path,
    frames: &[usize],
) -> Result<(usize, u64, String), String> {
    use trit_core::fingerprint::{file_sha256, sha256_hex};
    let mut lines = Vec::new();
    let mut bytes = 0u64;
    let mut files = 0usize;
    for &f in frames {
        for (sub, ext) in [("seg", "npy"), ("depth", "npy"), ("meta", "json")] {
            let p = frame_path(scene, sub, f, ext);
            let meta = std::fs::metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            bytes += meta.len();
            files += 1;
            lines.push(format!("{sub}/{f:06} {}", file_sha256(&p)?));
        }
    }
    Ok((files, bytes, sha256_hex(lines.join("\n").as_bytes())))
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
        write_ply(&p, &m).unwrap_or_else(|e| fail(&e));
        let bytes = std::fs::metadata(&p).map(|x| x.len()).unwrap_or(0);
        let sha = trit_core::fingerprint::file_sha256(&p).unwrap_or_default();
        let boundary = boundary_edges(&m);
        let nonmanifold = non_manifold_edges(&m);
        println!(
            "cell {cell} m: 顶点 {} 面 {} 边界边 {} 非流形 {} → {name}",
            m.verts.len(),
            m.tris.len(),
            boundary,
            nonmanifold
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
        });
    }

    let (files, data_bytes, data_hash) =
        scene_fingerprint(&scene, &frames).unwrap_or_else(|e| fail(&e));
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
                "{{\"cell\": {}, \"path\": {}, \"sha256\": \"{}\", \"bytes\": {}, \"verts\": {}, \"tris\": {}, \"boundary_edges\": {}, \"nonmanifold_edges\": {}}}",
                l.cell,
                trit_parts::partsrun::json_str(&l.name),
                l.sha256,
                l.bytes,
                l.verts,
                l.tris,
                l.boundary,
                l.nonmanifold
            )
        })
        .collect();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-asset\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"frames\": [{}], \"cells\": [{}]}},\n  \"data\": {{\"files\": {files}, \"bytes\": {data_bytes}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{data_hash}\", \"scene\": {}}},\n  \"model\": {{\"path\": {}, \"sha256\": \"{model_sha}\", \"bytes\": {model_bytes}}},\n  \"levels\": [{}]\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        frames_json.join(", "),
        cells_json.join(", "),
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&scene)),
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&model_path)),
        levels_json.join(", ")
    );
    let mp = out.join("asset.manifest.json");
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!("清单 {}", mp.display());
}
