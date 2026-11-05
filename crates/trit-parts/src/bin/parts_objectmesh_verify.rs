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

fn field(txt: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\": \"");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

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

fn block<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": {{");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let mut depth = 1usize;
    let (mut in_str, mut esc) = (false, false);
    for (j, ch) in rest.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
        } else if ch == '"' {
            in_str = true;
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(&rest[..j]);
            }
        }
    }
    None
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
        let lo = nums(seg, "bbox_min");
        let hi = nums(seg, "bbox_max");
        if lo.len() != 3 || hi.len() != 3 {
            continue;
        }
        out.push(Obj {
            id: seg
                .split(',')
                .next()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            bbox_min: [lo[0], lo[1], lo[2]],
            bbox_max: [hi[0], hi[1], hi[2]],
        });
    }
    out
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

fn entries(txt: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let n = |k: &str| number(seg, k).unwrap_or(0.0) as usize;
        out.push(Entry {
            id: seg
                .split(',')
                .next()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            obj: field(seg, "obj").unwrap_or_default(),
            obj_sha: field(seg, "obj_sha256").unwrap_or_default(),
            mtl: field(seg, "mtl").unwrap_or_default(),
            mtl_sha: field(seg, "mtl_sha256").unwrap_or_default(),
            png: field(seg, "png").unwrap_or_default(),
            png_sha: field(seg, "png_sha256").unwrap_or_default(),
            verts: n("verts"),
            tris: n("tris"),
            boundary: n("boundary"),
            nonmanifold: n("nonmanifold"),
            texels: n("texels"),
            unseen: n("unseen"),
            atlas: n("atlas"),
            charts: n("charts"),
        })
    }
    out
}

fn layout_rows(txt: &str) -> Vec<Placement> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let tr = nums(seg, "translate");
        let lo = nums(seg, "bbox_min");
        let hi = nums(seg, "bbox_max");
        if tr.len() != 3 || lo.len() != 3 || hi.len() != 3 {
            continue;
        }
        out.push(Placement {
            id: seg
                .split(',')
                .next()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            yaw: number(seg, "yaw").unwrap_or(0.0) as u32,
            translate: [tr[0], tr[1], tr[2]],
            bbox_min: [lo[0], lo[1], lo[2]],
            bbox_max: [hi[0], hi[1], hi[2]],
        });
    }
    out
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

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| {
        eprintln!("读不了 {}: {e}", p.display());
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
    let cfg = block(&manifest, "config").unwrap_or_else(|| usage());
    let inputs = block(&manifest, "inputs").unwrap_or_else(|| usage());
    let oman = block(inputs, "objects_manifest").unwrap_or_else(|| usage());
    let data = block(&manifest, "data").unwrap_or_else(|| usage());
    let scene = PathBuf::from(field(data, "scene").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = {
        let pat = "\"frames\": [";
        let i = cfg.find(pat).unwrap_or_else(|| usage()) + pat.len();
        let rest = &cfg[i..];
        let end = rest.find(']').unwrap_or(rest.len());
        rest[..end]
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    };
    let cell = number(cfg, "cell").unwrap_or(0.01) as f32;
    let atlas = number(cfg, "atlas").unwrap_or(512.0) as usize;
    let deg = number(cfg, "atlas_deg").unwrap_or(45.0) as f32;
    let want = entries(&manifest);

    let ompath = PathBuf::from(field(oman, "path").unwrap_or_else(|| usage()));
    let ok_oman = field(oman, "sha256")
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
    let ok_data = manifest.contains("\"algo\": \"sha256-content-v1\"")
        && field(data, "hash") == Some(format!("sha256:{}", fp.hash))
        && number(data, "files") == Some(fp.files as f64)
        && number(data, "bytes") == Some(fp.bytes as f64);

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
    let scene_block = block(&manifest, "scene");
    let layout_in = block(inputs, "layout");
    let mut ok_scene = true;
    if let Some(lb) = layout_in {
        let lp = PathBuf::from(field(lb, "path").unwrap_or_else(|| usage()));
        ok_scene &= field(lb, "sha256").map(|s| s == hash(&lp)).unwrap_or(false);
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
        ok_scene &= field(sb, "obj_sha256") == Some(hash(&tmp.join("scene.obj")))
            && field(sb, "mtl_sha256") == Some(hash(&tmp.join("scene.mtl")))
            && field(sb, "obj_sha256") == Some(hash(&base.join("scene.obj")))
            && field(sb, "mtl_sha256") == Some(hash(&base.join("scene.mtl")));
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
    let vp = custom.clone().unwrap_or_else(|| {
        base.join(if pass {
            "objects-mesh.verify.json"
        } else {
            "objects-mesh.verify.reject.json"
        })
    });
    if custom.is_none() {
        let _ = std::fs::remove_file(base.join(if pass {
            "objects-mesh.verify.reject.json"
        } else {
            "objects-mesh.verify.json"
        }));
    }
    std::fs::write(&vp, body).unwrap_or_else(|e| {
        eprintln!("写复核记录 {}: {e}", vp.display());
        std::process::exit(2);
    });
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
        let inputs = block(man, "inputs").unwrap();
        assert_eq!(
            field(block(inputs, "layout").unwrap(), "sha256").as_deref(),
            Some("lm")
        );
        let e = entries(man);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].id, 1);
        assert_eq!(e[0].obj_sha, "a");
        assert_eq!((e[0].verts, e[0].tris, e[0].boundary), (3, 1, 0));
        assert_eq!((e[0].texels, e[0].unseen, e[0].charts), (9, 1, 2));
        let sb = block(man, "scene").unwrap();
        assert_eq!(field(sb, "mtl_sha256").as_deref(), Some("sm"));
        assert_eq!(number(block(man, "config").unwrap(), "atlas"), Some(512.0));
    }

    #[test]
    fn stats_comparison_is_deciding() {
        let man = "{\"objects\": [{\"id\": 1, \"verts\": 3, \"tris\": 1, \"boundary\": 0, \"nonmanifold\": 0, \"texels\": 9, \"unseen\": 1, \"atlas\": 512, \"charts\": 2}]}";
        let e = entries(man).remove(0);
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
