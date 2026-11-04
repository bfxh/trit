//! `parts-asset-verify`：资产导出族（A2）的**独立复核**。
//!
//! 不信任清单：新进程重算模型 SHA 与单场景语料指纹，并把 `parts-3d export-asset`
//! 重跑到临时目录；逐档比对清单哈希、磁盘上交付件哈希与重导出哈希，外加顶点/
//! 面/边界/非流形/无观测顶点计数。全一致才写 `asset.verify.json`，否则写
//! `asset.verify.reject.json` 并退出码 1。
//!
//! ```text
//! parts-asset-verify --asset runs/assets/scene0084
//! ```

use std::path::{Path, PathBuf};

use trit_core::fingerprint::file_sha256;
use trit_parts::partsrun::{created_unix, git_out, json_str};
use trit_parts::scenefp::{fingerprint_scene_frames, norm_root};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!(
        "用法: parts-asset-verify --asset <目录> [--manifest <json>] \
         [--parts-3d <exe>] [--json-out <path>]"
    );
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

/// 取 `"key": { ... }` 的对象体（清单里的对象都不嵌套）。
fn block<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": {{");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    Some(&rest[..rest.find('}')?])
}

/// 取 `"key": [ ... ]` 的数组体（元素里没有 `]`）。
fn array<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": [");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    Some(&rest[..rest.find(']')?])
}

fn list_usize(txt: &str, key: &str) -> Option<Vec<usize>> {
    array(txt, key)?
        .split(',')
        .map(|s| s.trim().parse().ok())
        .collect()
}

fn list_f32(txt: &str, key: &str) -> Option<Vec<f32>> {
    array(txt, key)?
        .split(',')
        .map(|s| s.trim().parse().ok())
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct Lv {
    cell: f32,
    path: String,
    sha: String,
    verts: usize,
    tris: usize,
    boundary: usize,
    nonmanifold: usize,
    unseen: usize,
}

fn levels(txt: &str) -> Vec<Lv> {
    let mut out = Vec::new();
    for seg in txt.split("{\"cell\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let cell = seg
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(f32::NAN);
        let n = |k: &str| number(seg, k).unwrap_or(-1.0) as usize;
        out.push(Lv {
            cell,
            path: field(seg, "path").unwrap_or_default(),
            sha: field(seg, "sha256").unwrap_or_default(),
            verts: n("verts"),
            tris: n("tris"),
            boundary: n("boundary_edges"),
            nonmanifold: n("nonmanifold_edges"),
            unseen: n("unseen_vertices"),
        });
    }
    out
}

/// 清单声称的一档 vs 重导出的一档：几何计数与档位必须逐项相等。
fn levels_match(w: &Lv, g: &Lv) -> bool {
    w.path == g.path
        && (w.cell - g.cell).abs() <= 1e-6
        && w.verts == g.verts
        && w.tris == g.tris
        && w.boundary == g.boundary
        && w.nonmanifold == g.nonmanifold
        && w.unseen == g.unseen
}

fn sibling(name: &str) -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    if cfg!(windows) {
        dir.join(format!("{name}.exe"))
    } else {
        dir.join(name)
    }
}

fn read_manifest(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| {
        eprintln!("读不了清单 {}: {e}", p.display());
        std::process::exit(2);
    })
}

fn hash(p: &Path) -> String {
    file_sha256(p).unwrap_or_default()
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let asset = arg(&argv, "--asset").map(PathBuf::from);
    let mpath = match (arg(&argv, "--manifest"), &asset) {
        (Some(m), _) => PathBuf::from(m),
        (None, Some(a)) => a.join("asset.manifest.json"),
        (None, None) => usage(),
    };
    let asset_dir = mpath
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let bin = arg(&argv, "--parts-3d")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-3d"));
    let manifest = read_manifest(&mpath);

    let model_block = block(&manifest, "model").unwrap_or_else(|| usage());
    let data_block = block(&manifest, "data").unwrap_or_else(|| usage());
    let config_block = block(&manifest, "config").unwrap_or_else(|| usage());
    let model = PathBuf::from(field(model_block, "path").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(field(data_block, "scene").unwrap_or_else(|| usage()));
    let want_model = field(model_block, "sha256").unwrap_or_default();
    let want_data = field(data_block, "hash").unwrap_or_default();
    let want_files = number(data_block, "files").unwrap_or(-1.0);
    let want_bytes = number(data_block, "bytes").unwrap_or(-1.0);
    let frames = list_usize(config_block, "frames").unwrap_or_else(|| usage());
    let cells = list_f32(config_block, "cells").unwrap_or_else(|| usage());
    let want_levels = levels(array(&manifest, "levels").unwrap_or(""));

    let fp = fingerprint_scene_frames(&scene, &frames).unwrap_or_else(|e| {
        eprintln!("语料指纹失败: {e}");
        std::process::exit(2);
    });
    let model_sha = file_sha256(&model).unwrap_or_else(|e| {
        eprintln!("模型哈希失败: {e}");
        std::process::exit(2);
    });
    let ok_model = manifest.contains("\"kind\": \"extract-asset\"") && want_model == model_sha;
    let ok_data = manifest.contains("\"algo\": \"sha256-content-v1\"")
        && want_data == format!("sha256:{}", fp.hash)
        && want_files == fp.files as f64
        && want_bytes == fp.bytes as f64;

    // 复核不复用导出进程：重跑到临时目录，交付目录一个字节都不动。
    let tmp = asset_dir.join("verify.tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap_or_else(|e| fail(&format!("{}: {e}", tmp.display())));
    let join = |v: &[String]| v.join(",");
    let fs: Vec<String> = frames.iter().map(|x| x.to_string()).collect();
    let cs: Vec<String> = cells.iter().map(|x| x.to_string()).collect();
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("export-asset")
        .arg("--model")
        .arg(&model)
        .arg("--scene")
        .arg(&scene)
        .arg("--frames")
        .arg(join(&fs))
        .arg("--cells")
        .arg(join(&cs))
        .arg("--out")
        .arg(&tmp);
    let out = cmd.output().unwrap_or_else(|e| {
        eprintln!("跑不了 {}: {e}", bin.display());
        std::process::exit(2);
    });
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        println!("parts-3d export-asset 退出码 {:?}", out.status.code());
    }
    let got_levels =
        levels(array(&read_manifest(&tmp.join("asset.manifest.json")), "levels").unwrap_or(""));

    let mut matched = 0usize;
    let mut bad: Vec<String> = Vec::new();
    if out.status.success() && got_levels.len() == want_levels.len() {
        for (w, g) in want_levels.iter().zip(got_levels.iter()) {
            let delivered = hash(&asset_dir.join(&w.path));
            let redo = hash(&tmp.join(&g.path));
            if levels_match(w, g) && delivered == w.sha && redo == w.sha {
                matched += 1;
            } else {
                bad.push(w.path.clone());
            }
        }
    } else if want_levels.len() != got_levels.len() {
        bad.push(format!("档数 {} ≠ {}", want_levels.len(), got_levels.len()));
    }
    let ok_levels = out.status.success()
        && !want_levels.is_empty()
        && got_levels.len() == want_levels.len()
        && matched == want_levels.len();

    let pass = ok_model && ok_data && ok_levels;
    println!(
        "复核 {}：模型 {}｜语料 {}/{} 字节 {}｜档位 {}/{} 一致{}",
        mpath.display(),
        if ok_model { "一致" } else { "不一致" },
        fp.files,
        want_files as u64,
        fp.bytes,
        matched,
        want_levels.len(),
        if bad.is_empty() {
            String::new()
        } else {
            format!("（异常：{}）", bad.join(", "))
        }
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：模型 / 语料指纹 / 每档交付哈希与重导出全一致"
        } else {
            "复核 FAIL：与清单或重导出结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-asset-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"asset\": {{\"dir\": {}, \"manifest\": {{\"path\": {}, \"sha256\": \"{}\"}}}},\n  \"model\": {{\"path\": {}, \"sha256\": \"{model_sha}\", \"ok\": {ok_model}}},\n  \"data\": {{\"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"files\": {}, \"bytes\": {}, \"scene\": {}, \"ok\": {ok_data}}},\n  \"levels\": {{\"total\": {}, \"matched\": {matched}, \"bad\": [{}], \"ok\": {ok_levels}}},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&norm_root(&asset_dir)),
        json_str(&norm_root(&mpath)),
        hash(&mpath),
        json_str(&norm_root(&model)),
        fp.hash,
        fp.files,
        fp.bytes,
        json_str(&norm_root(&scene)),
        want_levels.len(),
        bad.iter()
            .map(|s| json_str(s))
            .collect::<Vec<_>>()
            .join(", "),
        if pass { "pass" } else { "reject" }
    );
    // 只有写默认位置时才维护 PASS/reject 互斥；--json-out 是旁路记录。
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        asset_dir.join(if pass {
            "asset.verify.json"
        } else {
            "asset.verify.reject.json"
        })
    });
    if custom.is_none() {
        let _ = std::fs::remove_file(asset_dir.join(if pass {
            "asset.verify.reject.json"
        } else {
            "asset.verify.json"
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

    const MAN: &str = "{\"kind\": \"extract-asset\", \"config\": {\"frames\": [0, 1], \"cells\": [0.01, 0.02]}, \"data\": {\"files\": 6, \"bytes\": 900, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:ab\", \"scene\": \"D:/r/scene0084\"}, \"model\": {\"path\": \"D:/m/n2.occ1\", \"sha256\": \"cd\", \"bytes\": 40}, \"levels\": [{\"cell\": 0.01, \"path\": \"lod-1cm.ply\", \"sha256\": \"e1\", \"bytes\": 10, \"verts\": 11, \"tris\": 12, \"boundary_edges\": 0, \"nonmanifold_edges\": 0, \"unseen_vertices\": 3}, {\"cell\": 0.02, \"path\": \"lod-2cm.ply\", \"sha256\": \"e2\", \"bytes\": 20, \"verts\": 21, \"tris\": 22, \"boundary_edges\": 0, \"nonmanifold_edges\": 0, \"unseen_vertices\": 4}]}";

    #[test]
    fn blocks_are_scoped_and_lists_parse() {
        // "path" 先出现在 model 里，不能被 levels 的 path 抢走
        assert_eq!(
            field(block(MAN, "model").unwrap(), "path").as_deref(),
            Some("D:/m/n2.occ1")
        );
        assert_eq!(
            field(block(MAN, "data").unwrap(), "scene").as_deref(),
            Some("D:/r/scene0084")
        );
        let cfg = block(MAN, "config").unwrap();
        assert_eq!(list_usize(cfg, "frames"), Some(vec![0, 1]));
        assert_eq!(list_f32(cfg, "cells"), Some(vec![0.01, 0.02]));
        assert_eq!(number(block(MAN, "data").unwrap(), "files"), Some(6.0));
        assert_eq!(list_f32(cfg, "nope"), None);
    }

    #[test]
    fn levels_parse_all_metrics() {
        let lv = levels(array(MAN, "levels").unwrap());
        assert_eq!(lv.len(), 2);
        assert_eq!(lv[0].cell, 0.01);
        assert_eq!(lv[0].path, "lod-1cm.ply");
        assert_eq!(lv[0].sha, "e1");
        assert_eq!(
            (lv[0].verts, lv[0].tris, lv[0].boundary, lv[0].unseen),
            (11, 12, 0, 3)
        );
        assert_eq!(lv[1].path, "lod-2cm.ply");
        assert_eq!(lv[1].unseen, 4);
        // 空数组/缺键都不许崩
        assert!(levels("").is_empty());
    }

    #[test]
    fn levels_match_catches_every_field() {
        let lv = levels(array(MAN, "levels").unwrap());
        assert!(levels_match(&lv[0], &lv[0]));
        let mut g = lv[0].clone();
        g.unseen = 0;
        assert!(!levels_match(&lv[0], &g), "无观测顶点数变了必须判红");
        let mut g = lv[0].clone();
        g.path = "lod-9cm.ply".into();
        assert!(!levels_match(&lv[0], &g));
        let mut g = lv[0].clone();
        g.cell = 0.02;
        assert!(!levels_match(&lv[0], &g));
    }
}
