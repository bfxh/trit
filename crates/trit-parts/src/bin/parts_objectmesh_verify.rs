//! `parts-objectmesh-verify`：对象级网格+贴图（A2）的**独立复核**。
//!
//! 重算语料指纹与输入清单 SHA，按同一套融合/外壳/图集参数**重建每个对象**并写到临时目录，
//! 逐文件比对 SHA（OBJ/MTL/PNG）与几何/图集统计；给布局时再重建 `scene.obj/scene.mtl`。
//! 不一致就写 `objects-mesh.verify.reject.json` 并退出码 1。
//!
//! ```text
//! parts-objectmesh-verify --dir runs/objects-mesh/scene0084
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use trit_learn::json::Json;
use trit_learn::verify::publish;
use trit_parts::data::{read_npy_i32, read_rgb, write_rgb};
use trit_parts::fuse::View;
use trit_parts::geo::{Camera, read_npy_f32};
use trit_parts::objmesh::{build_objects, write_scene};
use trit_parts::partsrun::{created_unix, git_out, json_str};
use trit_parts::scenefp::{fingerprint_scene_frames_rgb, norm_root};
use trit_parts::splat::{Item, Placement};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!("用法: parts-objectmesh-verify --dir <目录> [--json-out <path>]");
    std::process::exit(2);
}

fn num(j: &Json, key: &str) -> Option<f64> {
    j.get(key).and_then(Json::as_f64)
}

fn text(j: &Json, key: &str) -> Option<String> {
    j.get_str(key).map(|s| s.to_string())
}

fn nums3(j: &Json, key: &str) -> Vec<f64> {
    j.get(key)
        .and_then(Json::as_array)
        .map(|items| items.iter().filter_map(Json::as_f64).collect())
        .unwrap_or_default()
}

fn list_usize(j: &Json, key: &str) -> Vec<usize> {
    j.get(key)
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_u64().map(|n| n as usize))
                .collect()
        })
        .unwrap_or_default()
}

struct Obj {
    id: u32,
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
}

fn objects(v: &Json) -> Vec<Obj> {
    let Some(items) = v.get("objects").and_then(Json::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|e| {
            let lo = nums3(e, "bbox_min");
            let hi = nums3(e, "bbox_max");
            if lo.len() != 3 || hi.len() != 3 {
                return None;
            }
            Some(Obj {
                id: num(e, "id").unwrap_or(0.0) as u32,
                bbox_min: [lo[0], lo[1], lo[2]],
                bbox_max: [hi[0], hi[1], hi[2]],
            })
        })
        .collect()
}

struct Entry {
    id: u32,
    obj: String,
    obj_sha: String,
    mtl: String,
    mtl_sha: String,
    png: String,
    png_sha: String,
    verts: usize,
    tris: usize,
    boundary: usize,
    nonmanifold: usize,
    texels: usize,
    unseen: usize,
    atlas: usize,
    charts: usize,
}

fn entries(v: &Json) -> Vec<Entry> {
    let Some(items) = v.get("objects").and_then(Json::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|e| {
            let n = |k: &str| num(e, k).unwrap_or(0.0) as usize;
            Entry {
                id: num(e, "id").unwrap_or(0.0) as u32,
                obj: e.get_str("obj").unwrap_or_default().to_string(),
                obj_sha: e.get_str("obj_sha256").unwrap_or_default().to_string(),
                mtl: e.get_str("mtl").unwrap_or_default().to_string(),
                mtl_sha: e.get_str("mtl_sha256").unwrap_or_default().to_string(),
                png: e.get_str("png").unwrap_or_default().to_string(),
                png_sha: e.get_str("png_sha256").unwrap_or_default().to_string(),
                verts: n("verts"),
                tris: n("tris"),
                boundary: n("boundary"),
                nonmanifold: n("nonmanifold"),
                texels: n("texels"),
                unseen: n("unseen"),
                atlas: n("atlas"),
                charts: n("charts"),
            }
        })
        .collect()
}

fn layout_rows(v: &Json) -> Vec<Placement> {
    let Some(items) = v.get("layout").and_then(Json::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|e| {
            let tr = nums3(e, "translate");
            let lo = nums3(e, "bbox_min");
            let hi = nums3(e, "bbox_max");
            if tr.len() != 3 || lo.len() != 3 || hi.len() != 3 {
                return None;
            }
            Some(Placement {
                id: num(e, "id").unwrap_or(0.0) as u32,
                yaw: num(e, "yaw").unwrap_or(0.0) as u32,
                translate: [tr[0], tr[1], tr[2]],
                bbox_min: [lo[0], lo[1], lo[2]],
                bbox_max: [hi[0], hi[1], hi[2]],
            })
        })
        .collect()
}

fn resolve(base: &Path, p: String) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() { p } else { base.join(p) }
}

fn load_view(scene: &Path, f: usize) -> View {
    let (ids, shape) =
        read_npy_i32(&scene.join("seg").join(format!("{f:06}.npy"))).unwrap_or_else(|e| fail(&e));
    let (d, _) =
        read_npy_f32(&scene.join("depth").join(format!("{f:06}.npy"))).unwrap_or_else(|e| fail(&e));
    View {
        ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
        depth: d,
        w: shape[1],
        h: shape[0],
        cam: Camera::load(&scene.join("meta").join(format!("{f:06}.json")))
            .unwrap_or_else(|e| fail(&e)),
    }
}

fn load_rgb(scene: &Path, f: usize) -> Vec<f32> {
    read_rgb(&scene.join("rgb").join(format!("{f:06}.png")))
        .map(|(_, _, p)| p)
        .unwrap_or_else(|e| fail(&e))
}

fn hash(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

fn read(p: &Path) -> Json {
    trit_learn::json::read_json(p).unwrap_or_else(|e| {
        eprintln!("读不了 {e}");
        std::process::exit(2);
    })
}

#[allow(clippy::too_many_arguments)]
fn stats_eq(
    e: &Entry,
    verts: usize,
    tris: usize,
    boundary: usize,
    nonmanifold: usize,
    texels: usize,
    unseen: usize,
    atlas: usize,
    charts: usize,
) -> bool {
    e.verts == verts
        && e.tris == tris
        && e.boundary == boundary
        && e.nonmanifold == nonmanifold
        && e.texels == texels
        && e.unseen == unseen
        && e.atlas == atlas
        && e.charts == charts
}
fn stats_match(e: &Entry, b: &trit_parts::objmesh::Built) -> bool {
    stats_eq(
        e,
        b.mesh.verts.len(),
        b.mesh.tris.len(),
        b.boundary,
        b.nonmanifold,
        b.texels,
        b.unseen,
        b.atlas.w,
        b.atlas.charts.len(),
    )
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let dir = arg(&argv, "--dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| usage());
    let mpath = dir.join("objects-mesh.manifest.json");
    let base = mpath
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = read(&mpath);
    let cfg = manifest.get("config").unwrap_or_else(|| usage());
    let inputs = manifest.get("inputs").unwrap_or_else(|| usage());
    let oman = inputs.get("objects_manifest").unwrap_or_else(|| usage());
    let data = manifest.get("data").unwrap_or_else(|| usage());
    let scene = PathBuf::from(data.get_str("scene").unwrap_or_else(|| usage()));
    let frames = list_usize(cfg, "frames");
    let cell = num(cfg, "cell").unwrap_or(0.01) as f32;
    let atlas = num(cfg, "atlas").unwrap_or(512.0) as usize;
    let deg = num(cfg, "atlas_deg").unwrap_or(45.0) as f32;
    let want = entries(&manifest);

    let ompath = resolve(&base, text(oman, "path").unwrap_or_else(|| usage()));
    let ok_oman = text(oman, "sha256")
        .map(|s| s == hash(&ompath))
        .unwrap_or(false);
    let om = read(&ompath);
    let objs = objects(&om);
    let ids: Vec<u32> = objs.iter().map(|o| o.id).collect();
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

    let fp = fingerprint_scene_frames_rgb(&scene, &frames).unwrap_or_else(|e| {
        eprintln!("语料指纹失败: {e}");
        std::process::exit(2);
    });
    let want_hash = format!("sha256:{}", fp.hash);
    let ok_data = data.get_str("algo") == Some("sha256-content-v1")
        && data.get_str("hash") == Some(want_hash.as_str())
        && num(data, "files") == Some(fp.files as f64)
        && num(data, "bytes") == Some(fp.bytes as f64);

    let views: Vec<View> = frames.iter().map(|&f| load_view(&scene, f)).collect();
    let rgbs: Vec<Vec<f32>> = frames.iter().map(|&f| load_rgb(&scene, f)).collect();
    let built = build_objects(&views, &rgbs, cell, atlas, deg, &ids).unwrap_or_else(|e| fail(&e));
    let tmp = base.join("verify.tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap_or_else(|e| fail(&format!("{}: {e}", tmp.display())));

    let mut bad = Vec::new();
    for b in &built {
        let stem = format!("obj-{}", b.id);
        let png = tmp.join(format!("{stem}.png"));
        write_rgb(&png, b.atlas.w, b.atlas.h, &b.atlas.img).unwrap_or_else(|e| fail(&e));
        let obj = tmp.join(format!("{stem}.obj"));
        trit_parts::uv::write_obj(&obj, &b.mesh, &b.atlas.uv, &format!("{stem}.png"))
            .unwrap_or_else(|e| fail(&e));
        let mtl = tmp.join(format!("{stem}.mtl"));
        let e = want.iter().find(|e| e.id == b.id);
        let ok = e
            .map(|e| {
                e.obj_sha == hash(&obj)
                    && e.mtl_sha == hash(&mtl)
                    && e.png_sha == hash(&png)
                    && e.obj_sha == hash(&base.join(&e.obj))
                    && e.mtl_sha == hash(&base.join(&e.mtl))
                    && e.png_sha == hash(&base.join(&e.png))
                    && stats_match(e, b)
            })
            .unwrap_or(false);
        println!(
            "对象 {}: {} 顶点 / {} 面{}",
            b.id,
            b.mesh.verts.len(),
            b.mesh.tris.len(),
            if ok { "" } else { "  ← 不一致" }
        );
        if !ok {
            bad.push(b.id);
        }
    }
    let built_ids: std::collections::BTreeSet<u32> = built.iter().map(|b| b.id).collect();
    let want_ids: std::collections::BTreeSet<u32> = want.iter().map(|e| e.id).collect();
    let ok_set = built_ids == want_ids && !built.is_empty();

    // 场景 OBJ/MTL（可选）
    let scene_block = &manifest.get("scene");
    let layout_in = inputs.get("layout");
    let mut ok_scene = true;
    if let Some(lb) = layout_in {
        let lp = resolve(&base, text(lb, "path").unwrap_or_else(|| usage()));
        ok_scene &= text(lb, "sha256").map(|s| s == hash(&lp)).unwrap_or(false);
        let lt = read(&lp);
        let mut places: HashMap<u32, Placement> = HashMap::new();
        for pl in layout_rows(&lt) {
            places.insert(pl.id, pl);
        }
        ok_scene &= built.iter().all(|b| places.contains_key(&b.id));
        let parts: Vec<trit_parts::objmesh::ScenePart> = built
            .iter()
            .map(|b| (b.id, b.mesh.clone(), b.atlas.uv.clone()))
            .collect();
        write_scene(&tmp, &parts, &items, &places).unwrap_or_else(|e| fail(&e));
        let sb = scene_block.unwrap_or_else(|| usage());
        let obj_sha = hash(&tmp.join("scene.obj"));
        let mtl_sha = hash(&tmp.join("scene.mtl"));
        let base_obj = hash(&base.join("scene.obj"));
        let base_mtl = hash(&base.join("scene.mtl"));
        ok_scene &= sb.get_str("obj_sha256") == Some(obj_sha.as_str())
            && sb.get_str("mtl_sha256") == Some(mtl_sha.as_str())
            && sb.get_str("obj_sha256") == Some(base_obj.as_str())
            && sb.get_str("mtl_sha256") == Some(base_mtl.as_str());
    } else if scene_block.is_some() {
        ok_scene = false; // 清单说有场景但没布局输入
    }

    let pass = ok_oman && ok_data && ok_set && bad.is_empty() && ok_scene;
    println!(
        "复核 {}：对象清单 {}｜语料 {}｜对象 {}/{}{}｜场景 {}",
        mpath.display(),
        if ok_oman { "一致" } else { "不一致" },
        if ok_data { "一致" } else { "不一致" },
        built.len() - bad.len(),
        built.len(),
        if ok_set {
            ""
        } else {
            "（id 集合不一致）"
        },
        if ok_scene { "一致" } else { "不一致" }
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：输入清单 / 语料 / 逐对象 OBJ+MTL+PNG / 场景 OBJ+MTL 全一致"
        } else {
            "复核 FAIL：与清单或重建结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-objects-mesh-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"manifest\": {{\"path\": {}, \"sha256\": \"{}\"}},\n  \"inputs\": {{\"objects_manifest\": {{\"path\": {}, \"sha256\": \"{}\", \"ok\": {ok_oman}}}}},\n  \"data\": {{\"hash\": \"sha256:{}\", \"files\": {}, \"bytes\": {}, \"ok\": {ok_data}}},\n  \"objects\": {{\"total\": {}, \"bad\": [{}], \"set_ok\": {ok_set}}},\n  \"scene\": {{\"ok\": {ok_scene}}},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&norm_root(&mpath)),
        hash(&mpath),
        json_str(&norm_root(&ompath)),
        hash(&ompath),
        fp.hash,
        fp.files,
        fp.bytes,
        built.len(),
        bad.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        if pass { "pass" } else { "reject" }
    );
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp =
        publish(&base, "objects-mesh", pass, custom.as_deref(), &body).unwrap_or_else(|e| fail(&e));
    let _ = std::fs::remove_dir_all(&tmp);
    println!(
        "复核记录 {}（{}）",
        vp.display(),
        if pass { "pass" } else { "reject" }
    );
    if !pass {
        std::process::exit(1);
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("FAIL {msg}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_inputs_and_entries() {
        let man = "{\"config\": {\"frames\": [0, 1], \"cell\": 0.01, \"atlas\": 512, \"atlas_deg\": 45}, \"inputs\": {\"objects_manifest\": {\"path\": \"o/objects.manifest.json\", \"sha256\": \"om\"}, \"layout\": {\"path\": \"c/compose.manifest.json\", \"sha256\": \"lm\"}}, \"objects\": [{\"id\": 1, \"obj\": \"obj-1.obj\", \"obj_sha256\": \"a\", \"obj_bytes\": 10, \"mtl\": \"obj-1.mtl\", \"mtl_sha256\": \"b\", \"png\": \"obj-1.png\", \"png_sha256\": \"c\", \"png_bytes\": 20, \"verts\": 3, \"tris\": 1, \"boundary\": 0, \"nonmanifold\": 0, \"texels\": 9, \"unseen\": 1, \"atlas\": 512, \"charts\": 2}], \"scene\": {\"obj\": \"scene.obj\", \"obj_sha256\": \"so\", \"mtl\": \"scene.mtl\", \"mtl_sha256\": \"sm\", \"objects\": 1}}";
        let m = Json::parse(man).unwrap();
        let inputs = m.get("inputs").unwrap();
        assert_eq!(
            text(inputs.get("layout").unwrap(), "sha256").as_deref(),
            Some("lm")
        );
        let e = entries(&m);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].id, 1);
        assert_eq!(e[0].obj_sha, "a");
        assert_eq!((e[0].verts, e[0].tris, e[0].boundary), (3, 1, 0));
        assert_eq!((e[0].texels, e[0].unseen, e[0].charts), (9, 1, 2));
        let sb = m.get("scene").unwrap();
        assert_eq!(text(sb, "mtl_sha256").as_deref(), Some("sm"));
        assert_eq!(num(m.get("config").unwrap(), "atlas"), Some(512.0));
    }

    #[test]
    fn relative_inputs_resolve_against_bundle() {
        let base = Path::new("bundle/objects-mesh");
        assert_eq!(
            resolve(base, "../objects/objects.manifest.json".into()),
            PathBuf::from("bundle/objects-mesh/../objects/objects.manifest.json")
        );
        let abs = std::env::temp_dir().join("trit_om.json");
        assert_eq!(resolve(base, abs.display().to_string()), abs);
    }

    #[test]
    fn stats_comparison_is_deciding() {
        let man = "{\"objects\": [{\"id\": 1, \"verts\": 3, \"tris\": 1, \"boundary\": 0, \"nonmanifold\": 0, \"texels\": 9, \"unseen\": 1, \"atlas\": 512, \"charts\": 2}]}";
        let m = Json::parse(man).unwrap();
        let e = entries(&m).remove(0);
        assert!(stats_eq(&e, 3, 1, 0, 0, 9, 1, 512, 2));
        assert!(!stats_eq(&e, 4, 1, 0, 0, 9, 1, 512, 2), "顶点数变必须判红");
        assert!(
            !stats_eq(&e, 3, 1, 0, 0, 9, 2, 512, 2),
            "无观测纹素变必须判红"
        );
        assert!(
            !stats_eq(&e, 3, 1, 0, 0, 9, 1, 256, 2),
            "图集尺寸变必须判红"
        );
        assert!(
            !stats_eq(&e, 3, 1, 0, 0, 9, 1, 512, 3),
            "chart 数变必须判红"
        );
    }
}
