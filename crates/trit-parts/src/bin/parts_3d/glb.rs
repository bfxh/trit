//! `export-glb`：把对象网格包打成单个 GLB（glTF 2.0 二进制，PNG 内嵌、节点带布局）。
//!
//! ```text
//! parts-3d export-glb --objects-mesh <对象网格目录> --out <场景.glb> [--layout <compose.manifest.json>]
//! ```

use std::path::{Path, PathBuf};

use trit_parts::glb::{build_with, load_scene};

use super::cli::{arg, fail, usage};

/// 去掉 Windows `\\?\` 前缀（清单里存普通路径）。
fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest.to_string());
    }
    p
}

fn sha(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

pub(crate) fn export_glb_cmd(argv: &[String]) {
    let dir = PathBuf::from(arg(argv, "--objects-mesh").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let layout = arg(argv, "--layout").map(PathBuf::from);
    let generator = arg(argv, "--generator").unwrap_or_else(|| "trit-parts".into());
    let spin: f32 = arg(argv, "--spin")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let loaded = load_scene(&dir, layout.as_deref()).unwrap_or_else(|e| fail(&e));
    let bytes = build_with(
        &loaded.parts,
        &generator,
        if spin > 0.0 { Some(spin) } else { None },
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| fail(&format!("{}: {e}", parent.display())));
    }
    std::fs::write(&out, &bytes).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    println!(
        "GLB {}：{} 个对象 / {} 顶点 / {} 面 / {} B",
        out.display(),
        loaded.parts.len(),
        loaded.verts,
        loaded.tris,
        bytes.len()
    );

    let glb_dir = out
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let glb_dir = plain(glb_dir.canonicalize().unwrap_or(glb_dir));
    let rel = |p: &Path| {
        let q = plain(p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
        trit_parts::partsrun::rel_path(&glb_dir, &q)
    };
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
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-glb\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"generator\": {}, \"spin\": {spin}}},\n  \"inputs\": {{\"objects_mesh\": {{\"path\": {}, \"sha256\": \"{}\"}}, \"objects_manifest\": {{\"path\": {}, \"sha256\": \"{}\"}}, \"layout\": {{\"path\": {}, \"sha256\": \"{}\"}}}},\n  \"glb\": {{\"path\": {}, \"sha256\": \"{}\", \"bytes\": {}, \"parts\": {}, \"verts\": {}, \"tris\": {}}}\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        trit_parts::partsrun::json_str(&generator),
        trit_parts::partsrun::json_str(&rel(&loaded.mesh_manifest)),
        sha(&loaded.mesh_manifest),
        trit_parts::partsrun::json_str(&rel(&loaded.objects_manifest)),
        sha(&loaded.objects_manifest),
        trit_parts::partsrun::json_str(&rel(&loaded.layout)),
        sha(&loaded.layout),
        trit_parts::partsrun::json_str(
            out.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("scene.glb")
        ),
        sha(&out),
        bytes.len(),
        loaded.parts.len(),
        loaded.verts,
        loaded.tris
    );
    let mut mp = out.as_os_str().to_owned();
    mp.push(".manifest.json");
    let mp = PathBuf::from(mp);
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!("清单 {}", mp.display());
}
