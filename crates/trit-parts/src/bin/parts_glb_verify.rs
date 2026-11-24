//! `parts-glb-verify`：GLB 资产（A2）的**独立复核**。
//!
//! 重算三个输入清单的 SHA，按同一套 OBJ/PNG/布局**重新打包**一遍，与盘上的 GLB 逐字节比对；
//! 同时检查 GLB 头/分块与对象数、PNG 数。不一致就写 `.verify.reject.json` 并退出码 1。
//!
//! ```text
//! parts-glb-verify --glb runs/scene-full/scene0084/objects-mesh/scene.glb
//! ```

use std::path::{Path, PathBuf};

use trit_learn::json::Json;
use trit_learn::verify::publish_base;
use trit_parts::glb::{build_with, load_scene};
use trit_parts::partsrun::{created_unix, git_out, json_str};

// 二进制 crate root 的 `mod x;` 只找同目录 `x.rs`（同目录下每个 .rs 都会被 Cargo 当成 bin）。
#[path = "parts_glb_verify/chunks.rs"]
mod chunks;
use chunks::{chunks, png_count};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!("用法: parts-glb-verify --glb <路径> [--json-out <path>]");
    std::process::exit(2);
}

fn num(j: &Json, key: &str) -> Option<f64> {
    j.get(key).and_then(Json::as_f64)
}

fn sha_ok(j: &Json, p: &Path) -> bool {
    j.get_str("sha256").is_some_and(|s| s == sha(p))
}

fn resolve(base: &Path, p: String) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() { p } else { base.join(p) }
}

fn sha(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

fn read(p: &Path) -> Json {
    trit_learn::json::read_json(p).unwrap_or_else(|e| {
        trit_log::error!("读不了 {e}");
        std::process::exit(2);
    })
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let glb = PathBuf::from(arg(&argv, "--glb").unwrap_or_else(|| usage()));
    let mut mp = glb.as_os_str().to_owned();
    mp.push(".manifest.json");
    let mpath = PathBuf::from(mp);
    let base = glb
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = read(&mpath);
    let cfg = manifest.get("config").unwrap_or_else(|| usage());
    let inputs = manifest.get("inputs").unwrap_or_else(|| usage());
    let gb = manifest.get("glb").unwrap_or_else(|| usage());
    let generator = cfg.get_str("generator").unwrap_or("trit-parts").to_string();
    let spin = num(cfg, "spin").unwrap_or(0.0) as f32;
    let mesh_in = inputs.get("objects_mesh").unwrap_or_else(|| usage());
    let obj_in = inputs.get("objects_manifest").unwrap_or_else(|| usage());
    let lay_in = inputs.get("layout").unwrap_or_else(|| usage());
    let path = |j: &Json| j.get_str("path").unwrap_or_else(|| usage()).to_string();
    let mesh_p = resolve(&base, path(mesh_in));
    let obj_p = resolve(&base, path(obj_in));
    let lay_p = resolve(&base, path(lay_in));
    let ok_inputs = sha_ok(mesh_in, &mesh_p) && sha_ok(obj_in, &obj_p) && sha_ok(lay_in, &lay_p);
    let (want_parts, want_verts, want_tris) = (
        num(gb, "parts").unwrap_or(-1.0) as usize,
        num(gb, "verts").unwrap_or(-1.0) as usize,
        num(gb, "tris").unwrap_or(-1.0) as usize,
    );
    let mesh_dir = mesh_p.parent().unwrap_or_else(|| usage());
    let loaded = load_scene(mesh_dir, Some(&lay_p)).unwrap_or_else(|e| fail(&e));
    let ok_paths = loaded.objects_manifest == obj_p || sha(&loaded.objects_manifest) == sha(&obj_p);
    let rebuilt = build_with(
        &loaded.parts,
        &generator,
        if spin > 0.0 { Some(spin) } else { None },
    );
    let delivered = std::fs::read(&glb).unwrap_or_default();
    let ok_bytes = delivered == rebuilt
        && gb.get_str("sha256").is_some_and(|s| s == sha(&glb))
        && num(gb, "bytes") == Some(delivered.len() as f64);
    let ok_stats =
        want_parts == loaded.parts.len() && want_verts == loaded.verts && want_tris == loaded.tris;
    let (ok_struct, detail) = match chunks(&delivered) {
        Ok((json, bin)) => {
            let nodes = json.matches("\"mesh\": ").count();
            let pngs = png_count(bin);
            let anims = json.matches("\"animations\": [").count();
            let want_anim = usize::from(spin > 0.0);
            let ok_anim = anims == want_anim
                && (spin <= 0.0
                    || (json.contains("\"name\": \"turntable\"")
                        && json.contains("\"path\": \"rotation\"")));
            (
                json.contains("\"meshes\": [")
                    && nodes == loaded.parts.len()
                    && pngs == loaded.parts.len()
                    && ok_anim,
                format!("节点 {nodes}、PNG {pngs}、动画 {anims}"),
            )
        }
        Err(e) => (false, e),
    };

    let pass = ok_inputs && ok_paths && ok_bytes && ok_stats && ok_struct;
    println!(
        "复核 {}：输入 {}｜对象 {}/{}｜顶点 {}/{}｜字节 {}｜结构（{}）",
        mpath.display(),
        if ok_inputs && ok_paths {
            "一致"
        } else {
            "不一致"
        },
        loaded.parts.len(),
        want_parts,
        loaded.verts,
        want_verts,
        if ok_bytes {
            "逐字节一致"
        } else {
            "不一致"
        },
        detail
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：输入清单 / 逐字节重建 / 读回结构全一致"
        } else {
            "复核 FAIL：与清单或重建结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-glb-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"glb\": {{\"path\": {}, \"sha256\": \"{}\", \"bytes\": {}}},\n  \"inputs_ok\": {ok_inputs},\n  \"parts\": {{\"got\": {}, \"want\": {want_parts}, \"ok\": {ok_stats}}},\n  \"bytes_ok\": {ok_bytes},\n  \"struct_ok\": {ok_struct},\n  \"spin\": {spin},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&trit_parts::scenefp::norm_root(&glb)),
        sha(&glb),
        delivered.len(),
        loaded.parts.len(),
        if pass { "pass" } else { "reject" }
    );
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = publish_base(&glb, pass, custom.as_deref(), &body).unwrap_or_else(|e| fail(&e));
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
    fn manifest_fields_parse() {
        let man = r#"{"config": {"generator": "trit-parts"}, "inputs": {"objects_mesh": {"path": "a.json", "sha256": "m"}, "objects_manifest": {"path": "../o.json", "sha256": "o"}, "layout": {"path": "../l.json", "sha256": "l"}}, "glb": {"path": "s.glb", "sha256": "g", "bytes": 10, "parts": 7, "verts": 100, "tris": 50}}"#;
        let m = Json::parse(man).unwrap();
        let g = m.get("glb").unwrap();
        assert_eq!((num(g, "parts"), num(g, "verts")), (Some(7.0), Some(100.0)));
        let inp = m.get("inputs").unwrap();
        assert_eq!(inp.get("layout").unwrap().get_str("sha256"), Some("l"));
        assert_eq!(
            m.get("config").unwrap().get_str("generator"),
            Some("trit-parts")
        );
    }
}
