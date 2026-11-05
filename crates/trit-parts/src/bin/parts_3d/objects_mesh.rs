//! `export-object-meshes`：对象级网格 + 贴图（引擎可直接用的可编辑场景）。
//!
//! ```text
//! parts-3d export-object-meshes --scene <renders> --objects <对象目录> --out <目录> \
//!   [--frames 0,...,7] [--cell 0.01] [--atlas 512] [--atlas-deg 45] \
//!   [--layout <compose.manifest.json>]
//! ```
//!
//! 每对象一份 `obj-<id>.obj/.mtl/.png`（局部坐标，可单独编辑）；给 `--layout` 时再写
//! `scene.obj/scene.mtl`（按合成布局的 yaw/平移变换顶点，`g obj-<id>` 分组，保留对象边界）。

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use trit_parts::data::{read_npy_i32, read_rgb, write_rgb};
use trit_parts::fuse::View;
use trit_parts::geo::{Camera, read_npy_f32};
use trit_parts::objmesh::build_objects;
use trit_parts::splat::{Item, Placement};

use super::cli::{arg, fail, frame_path, usage};

fn number(txt: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\": ");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn nums(txt: &str, key: &str) -> Vec<f64> {
    let pat = format!("\"{key}\": [");
    let Some(i) = txt.find(&pat) else {
        return Vec::new();
    };
    let rest = &txt[i + pat.len()..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end]
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

struct Obj {
    id: u32,
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
}

fn objects(txt: &str) -> Vec<Obj> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let id = seg
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let (lo, hi) = (nums(seg, "bbox_min"), nums(seg, "bbox_max"));
        if lo.len() != 3 || hi.len() != 3 {
            continue;
        }
        out.push(Obj {
            id,
            bbox_min: [lo[0], lo[1], lo[2]],
            bbox_max: [hi[0], hi[1], hi[2]],
        });
    }
    out
}

fn layout_rows(txt: &str) -> Vec<Placement> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let id = seg
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let tr = nums(seg, "translate");
        let lo = nums(seg, "bbox_min");
        let hi = nums(seg, "bbox_max");
        if tr.len() != 3 || lo.len() != 3 || hi.len() != 3 {
            continue;
        }
        out.push(Placement {
            id,
            yaw: number(seg, "yaw").unwrap_or(0.0) as u32,
            translate: [tr[0], tr[1], tr[2]],
            bbox_min: [lo[0], lo[1], lo[2]],
            bbox_max: [hi[0], hi[1], hi[2]],
        });
    }
    out
}

fn load_view(scene: &std::path::Path, f: usize) -> View {
    let (ids, shape) =
        read_npy_i32(&frame_path(scene, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
    let (d, _) = read_npy_f32(&frame_path(scene, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
    View {
        ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
        depth: d,
        w: shape[1],
        h: shape[0],
        cam: Camera::load(&frame_path(scene, "meta", f, "json")).unwrap_or_else(|e| fail(&e)),
    }
}

fn load_rgb(scene: &std::path::Path, f: usize) -> Vec<f32> {
    read_rgb(&frame_path(scene, "rgb", f, "png"))
        .map(|(_, _, p)| p)
        .unwrap_or_else(|e| fail(&e))
}

fn sha(p: &std::path::Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

fn bytes(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

pub(crate) fn objects_mesh_cmd(argv: &[String]) {
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let objdir = PathBuf::from(arg(argv, "--objects").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let atlas_px: usize = arg(argv, "--atlas")
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let deg: f32 = arg(argv, "--atlas-deg")
        .and_then(|v| v.parse().ok())
        .unwrap_or(45.0);
    if frames.is_empty() {
        fail("--frames 不能为空");
    }
    let ompath = objdir.join("objects.manifest.json");
    let om = std::fs::read_to_string(&ompath)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", ompath.display())));
    let objs = objects(&om);
    let ids: Vec<u32> = objs.iter().map(|o| o.id).collect();
    if ids.is_empty() {
        fail("对象清单为空");
    }
    std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    let out = out
        .canonicalize()
        .unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));

    let views: Vec<View> = frames.iter().map(|&f| load_view(&scene, f)).collect();
    let rgbs: Vec<Vec<f32>> = frames.iter().map(|&f| load_rgb(&scene, f)).collect();
    let built =
        build_objects(&views, &rgbs, cell, atlas_px, deg, &ids).unwrap_or_else(|e| fail(&e));

    // 布局（可选）：文件里的 yaw/平移直接作用到网格顶点
    let layout_path = arg(argv, "--layout").map(PathBuf::from);
    let mut places: HashMap<u32, Placement> = HashMap::new();
    if let Some(lp) = &layout_path {
        let lt =
            std::fs::read_to_string(lp).unwrap_or_else(|e| fail(&format!("{}: {e}", lp.display())));
        for pl in layout_rows(&lt) {
            places.insert(pl.id, pl);
        }
    }
    let items: HashMap<u32, Item> = objs
        .iter()
        .map(|o| {
            (
                o.id,
                Item {
                    id: o.id,
                    bbox_min: o.bbox_min,
                    bbox_max: o.bbox_max,
                },
            )
        })
        .collect();

    let mut entries = Vec::new();
    let mut scene_parts = Vec::new();
    for b in &built {
        let stem = format!("obj-{}", b.id);
        let png = out.join(format!("{stem}.png"));
        write_rgb(&png, b.atlas.w, b.atlas.h, &b.atlas.img).unwrap_or_else(|e| fail(&e));
        let obj = out.join(format!("{stem}.obj"));
        trit_parts::uv::write_obj(&obj, &b.mesh, &b.atlas.uv, &format!("{stem}.png"))
            .unwrap_or_else(|e| fail(&e));
        let mtl = out.join(format!("{stem}.mtl"));
        println!(
            "对象 {}: 顶点 {} 面 {} 边界边 {} 非流形 {} 图集 {}²（纹素 {}，无观测 {}）",
            b.id,
            b.mesh.verts.len(),
            b.mesh.tris.len(),
            b.boundary,
            b.nonmanifold,
            b.atlas.w,
            b.texels,
            b.unseen
        );
        entries.push(format!(
            "{{\"id\": {}, \"obj\": {}, \"obj_sha256\": \"{}\", \"obj_bytes\": {}, \"mtl\": {}, \"mtl_sha256\": \"{}\", \"png\": {}, \"png_sha256\": \"{}\", \"png_bytes\": {}, \"verts\": {}, \"tris\": {}, \"boundary\": {}, \"nonmanifold\": {}, \"texels\": {}, \"unseen\": {}, \"atlas\": {}, \"charts\": {}}}",
            b.id,
            trit_parts::partsrun::json_str(&format!("{stem}.obj")),
            sha(&obj),
            bytes(&obj),
            trit_parts::partsrun::json_str(&format!("{stem}.mtl")),
            sha(&mtl),
            trit_parts::partsrun::json_str(&format!("{stem}.png")),
            sha(&png),
            bytes(&png),
            b.mesh.verts.len(),
            b.mesh.tris.len(),
            b.boundary,
            b.nonmanifold,
            b.texels,
            b.unseen,
            b.atlas.w,
            b.atlas.charts.len()
        ));
        scene_parts.push((b.id, b.mesh.clone(), b.atlas.uv.clone()));
    }
    if built.is_empty() {
        fail("没有任何对象出网格");
    }

    let mut scene_json = "null".to_string();
    let layout_meta = if let Some(lp) = &layout_path {
        let missing: BTreeSet<u32> = built
            .iter()
            .map(|b| b.id)
            .filter(|id| !places.contains_key(id))
            .collect();
        if !missing.is_empty() {
            fail(&format!("布局缺对象 {missing:?}"));
        }
        trit_parts::objmesh::write_scene(&out, &scene_parts, &items, &places)
            .unwrap_or_else(|e| fail(&e));
        let (so, sm) = (out.join("scene.obj"), out.join("scene.mtl"));
        scene_json = format!(
            "{{\"obj\": \"scene.obj\", \"obj_sha256\": \"{}\", \"mtl\": \"scene.mtl\", \"mtl_sha256\": \"{}\", \"objects\": {}}}",
            sha(&so),
            sha(&sm),
            built.len()
        );
        format!(
            "{{\"path\": {}, \"sha256\": \"{}\"}}",
            trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(lp)),
            sha(lp)
        )
    } else {
        "null".into()
    };

    let fp = trit_parts::scenefp::fingerprint_scene_frames_rgb(&scene, &frames)
        .unwrap_or_else(|e| fail(&e));
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
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-objects-mesh\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"frames\": [{}], \"cell\": {cell}, \"atlas\": {atlas_px}, \"atlas_deg\": {deg}}},\n  \"inputs\": {{\"objects_manifest\": {{\"path\": {}, \"sha256\": \"{}\"}}, \"layout\": {layout_meta}}},\n  \"data\": {{\"files\": {}, \"bytes\": {}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"scene\": {}}},\n  \"objects\": [{}],\n  \"scene\": {scene_json}\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        frames_json.join(", "),
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&ompath)),
        sha(&ompath),
        fp.files,
        fp.bytes,
        fp.hash,
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&scene)),
        entries.join(", ")
    );
    let mp = out.join("objects-mesh.manifest.json");
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!("清单 {}", mp.display());
}
