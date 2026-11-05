//! `parts-scene-verify`：场景包（A2）的**独立复核**。
//!
//! 总清单只记子清单的哈希与读数：本命令先核对这些哈希，再把 `parts-asset-verify` /
//! `parts-splat-verify` 各起一个新进程跑一遍（它们自己会重算语料指纹、重导出/重渲）。
//! 任一步不一致就写 `scene.verify.reject.json` 并退出码 1。
//!
//! ```text
//! parts-scene-verify --scene runs/scene/scene0084
//! ```

use std::path::{Path, PathBuf};

use trit_parts::partsrun::{created_unix, git_out, json_str};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!(
        "用法: parts-scene-verify --scene <目录> [--manifest <json>] \
         [--parts-asset-verify <exe>] [--parts-splat-verify <exe>] [--json-out <path>]"
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

/// 取 `"key": { ... }` 的对象体（支持嵌套，跳过字符串）。
fn object<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
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

fn count_entries(txt: &str, key: &str) -> usize {
    let pat = format!("\"{key}\": [");
    let Some(i) = txt.find(&pat) else {
        return 0;
    };
    let rest = &txt[i + pat.len()..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end].matches("{\"cell\": ").count()
}

/// 取 `"key": [ ... ]` 的数组体（支持嵌套，跳过字符串）。
fn array_body<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": [");
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
        } else if ch == '[' {
            depth += 1;
        } else if ch == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(&rest[..j]);
            }
        }
    }
    None
}

fn count_ids(txt: &str, key: &str) -> usize {
    array_body(txt, key)
        .map(|b| b.matches("{\"id\": ").count())
        .unwrap_or(0)
}

fn sha_matches(want: &str, got: &str) -> bool {
    !want.is_empty() && want == got
}

/// 子清单路径按总清单所在目录解析（相对路径 ⇒ 整个包可搬移，搬走后仍能复核）。
fn resolve(base: &Path, p: String) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() { p } else { base.join(p) }
}

fn counts_match(want: usize, got: usize) -> bool {
    want > 0 && want == got
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

fn hash(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| {
        eprintln!("读不了 {}: {e}", p.display());
        std::process::exit(2);
    })
}

fn run(bin: &Path, args: &[String], what: &str) -> std::process::Output {
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| {
            eprintln!("跑不了 {}: {e}", bin.display());
            std::process::exit(2);
        });
    println!("{what} 已跑（rc={:?}）", out.status.code());
    out
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let dir = arg(&argv, "--scene").map(PathBuf::from);
    let mpath = match (arg(&argv, "--manifest"), &dir) {
        (Some(m), _) => PathBuf::from(m),
        (None, Some(d)) => d.join("scene.manifest.json"),
        (None, None) => usage(),
    };
    let scene_dir = mpath
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let asset_verify = arg(&argv, "--parts-asset-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-asset-verify"));
    let splat_verify = arg(&argv, "--parts-splat-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-splat-verify"));
    let objects_verify = arg(&argv, "--objects-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-objects-verify"));
    let compose_verify = arg(&argv, "--compose-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-compose-verify"));
    let omesh_verify = arg(&argv, "--objectmesh-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-objectmesh-verify"));
    let preview_verify = arg(&argv, "--preview-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-preview-verify"));
    let manifest = read(&mpath);
    let parts = object(&manifest, "parts").unwrap_or_else(|| usage());
    let mesh = object(parts, "mesh").unwrap_or_else(|| usage());
    let splat = object(parts, "splat").unwrap_or_else(|| usage());
    let mman = object(mesh, "manifest").unwrap_or_else(|| usage());
    let sman = object(splat, "manifest").unwrap_or_else(|| usage());
    let mpath_sub = resolve(&scene_dir, field(mman, "path").unwrap_or_else(|| usage()));
    let spath_sub = resolve(&scene_dir, field(sman, "path").unwrap_or_else(|| usage()));
    let msha = field(mman, "sha256").unwrap_or_default();
    let ssha = field(sman, "sha256").unwrap_or_default();
    let want_levels = number(mesh, "levels").unwrap_or(-1.0) as usize;
    let want_splats = number(splat, "count").unwrap_or(-1.0) as usize;

    let mtxt = read(&mpath_sub);
    let stxt = read(&spath_sub);
    let sblock = object(&stxt, "splat").unwrap_or_else(|| usage());
    let ply = spath_sub
        .parent()
        .map(|d| d.join(field(sblock, "path").unwrap_or_else(|| usage())))
        .unwrap_or_else(|| usage());
    let ok_man = sha_matches(&msha, &hash(&mpath_sub)) && sha_matches(&ssha, &hash(&spath_sub));
    let got_levels = count_entries(&mtxt, "levels");
    let got_splats = number(&stxt, "count").unwrap_or(-1.0) as usize;
    let ok_counts = counts_match(want_levels, got_levels) && counts_match(want_splats, got_splats);

    // 子复核各起一个新进程：它们会重算语料指纹并重导出/重渲。
    let mout = run(
        &asset_verify,
        &[
            "--asset".into(),
            mpath_sub
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ],
        "parts-asset-verify",
    );
    let sout = run(
        &splat_verify,
        &["--splat".into(), ply.display().to_string()],
        "parts-splat-verify",
    );
    let mverdict = mpath_sub
        .parent()
        .map(|p| p.join("asset.verify.json"))
        .filter(|p| p.is_file())
        .map(|p| read(&p))
        .unwrap_or_default();
    let sverdict = {
        let mut s = ply.as_os_str().to_owned();
        s.push(".verify.json");
        let p = PathBuf::from(s);
        if p.is_file() { read(&p) } else { String::new() }
    };
    let ok_sub = mout.status.success()
        && sout.status.success()
        && mverdict.contains("\"verdict\": \"pass\"")
        && sverdict.contains("\"verdict\": \"pass\"");

    // `--full` 的四个子包（旧清单没有就跳过）
    let mut extra_ok = true;
    let mut extra = String::new();
    if let Some(ob) = object(parts, "objects") {
        let man = object(ob, "manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, field(man, "path").unwrap_or_else(|| usage()));
        let d = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| scene_dir.clone());
        let want = number(ob, "count").unwrap_or(-1.0) as usize;
        let got = count_ids(&read(&p), "objects");
        let o = run(
            &objects_verify,
            &["--objects".into(), d.display().to_string()],
            "parts-objects-verify",
        );
        let v = d.join("objects.verify.json");
        let ok = sha_matches(&field(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, got)
            && o.status.success()
            && v.is_file()
            && read(&v).contains("\"verdict\": \"pass\"");
        extra_ok &= ok;
        extra.push_str(&format!(" 对象 {got}/{want}="));
        extra.push_str(if ok { "ok" } else { "FAIL" });
    }
    if let Some(cb) = object(parts, "compose") {
        let man = object(cb, "manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, field(man, "path").unwrap_or_else(|| usage()));
        let d = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| scene_dir.clone());
        let ctxt = read(&p);
        let composed = object(&ctxt, "composed")
            .map(|b| number(b, "count").unwrap_or(-1.0))
            .unwrap_or(-1.0) as usize;
        let cov = object(&ctxt, "gate")
            .map(|b| number(b, "coverage").unwrap_or(f64::NAN))
            .unwrap_or(f64::NAN);
        let want = number(cb, "count").unwrap_or(-1.0) as usize;
        let want_cov = number(cb, "coverage").unwrap_or(f64::NAN);
        let o = run(
            &compose_verify,
            &["--scene".into(), d.display().to_string()],
            "parts-compose-verify",
        );
        let v = d.join("compose.verify.json");
        let ok = sha_matches(&field(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, composed)
            && (want_cov - cov).abs() <= 1e-9
            && o.status.success()
            && v.is_file()
            && read(&v).contains("\"verdict\": \"pass\"");
        extra_ok &= ok;
        extra.push_str(&format!(" 合成 {composed}/{want}="));
        extra.push_str(if ok { "ok" } else { "FAIL" });
    }
    if let Some(mb) = object(parts, "objects_mesh") {
        let man = object(mb, "manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, field(man, "path").unwrap_or_else(|| usage()));
        let d = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| scene_dir.clone());
        let want = number(mb, "count").unwrap_or(-1.0) as usize;
        let got = count_ids(&read(&p), "objects");
        let o = run(
            &omesh_verify,
            &["--dir".into(), d.display().to_string()],
            "parts-objectmesh-verify",
        );
        let v = d.join("objects-mesh.verify.json");
        let ok = sha_matches(&field(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, got)
            && o.status.success()
            && v.is_file()
            && read(&v).contains("\"verdict\": \"pass\"");
        extra_ok &= ok;
        extra.push_str(&format!(" 对象网格 {got}/{want}="));
        extra.push_str(if ok { "ok" } else { "FAIL" });
    }
    if let Some(pb) = object(parts, "preview") {
        let man = object(pb, "manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, field(man, "path").unwrap_or_else(|| usage()));
        let d = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| scene_dir.clone());
        let want = number(pb, "views").unwrap_or(-1.0) as usize;
        let got = object(&read(&p), "config")
            .map(|c| number(c, "views").unwrap_or(-1.0))
            .unwrap_or(-1.0) as usize;
        let o = run(
            &preview_verify,
            &["--preview".into(), d.display().to_string()],
            "parts-preview-verify",
        );
        let v = d.join("preview.verify.json");
        let ok = sha_matches(&field(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, got)
            && o.status.success()
            && v.is_file()
            && read(&v).contains("\"verdict\": \"pass\"");
        extra_ok &= ok;
        extra.push_str(&format!(" 预览 {got}/{want}="));
        extra.push_str(if ok { "ok" } else { "FAIL" });
    }

    let pass = ok_man && ok_counts && ok_sub && extra_ok;
    println!(
        "复核 {}：子清单 {}｜档数 {got_levels}/{want_levels}｜高斯 {got_splats}/{want_splats}｜子复核 {}｜子包{}",
        mpath.display(),
        if ok_man { "一致" } else { "不一致" },
        if ok_sub { "全 PASS" } else { "未通过" },
        if extra.is_empty() {
            "无".to_string()
        } else {
            extra.clone()
        }
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：总清单 / 子清单哈希 / 读数 / 两个子复核全一致"
        } else {
            "复核 FAIL：与总清单或子复核结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-scene-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"scene\": {{\"dir\": {}, \"manifest\": {{\"path\": {}, \"sha256\": \"{}\"}}}},\n  \"mesh\": {{\"manifest\": {}, \"levels\": {got_levels}, \"want_levels\": {want_levels}, \"ok\": {ok_man}}},\n  \"splat\": {{\"manifest\": {}, \"count\": {got_splats}, \"want_count\": {want_splats}, \"ok\": {ok_counts}}},\n  \"sub_verify_ok\": {ok_sub},\n  \"full_ok\": {extra_ok},\n  \"sub\": {},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&trit_parts::scenefp::norm_root(&scene_dir)),
        json_str(&trit_parts::scenefp::norm_root(&mpath)),
        hash(&mpath),
        json_str(&trit_parts::scenefp::norm_root(&mpath_sub)),
        json_str(&trit_parts::scenefp::norm_root(&ply)),
        json_str(extra.trim()),
        if pass { "pass" } else { "reject" }
    );
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        scene_dir.join(if pass {
            "scene.verify.json"
        } else {
            "scene.verify.reject.json"
        })
    });
    if custom.is_none() {
        let _ = std::fs::remove_file(scene_dir.join(if pass {
            "scene.verify.reject.json"
        } else {
            "scene.verify.json"
        }));
    }
    std::fs::write(&vp, body).unwrap_or_else(|e| {
        eprintln!("写复核记录 {}: {e}", vp.display());
        std::process::exit(2);
    });
    println!(
        "复核记录 {}（{}）",
        vp.display(),
        if pass { "pass" } else { "reject" }
    );
    if !pass {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAN: &str = "{\"kind\": \"extract-scene\", \"config\": {\"cells\": [0.01, 0.02]}, \"scene\": \"D:/r/scene0084\", \"parts\": {\"mesh\": {\"manifest\": {\"path\": \"D:/o/mesh/asset.manifest.json\", \"sha256\": \"m1\"}, \"levels\": 3, \"texture\": true}, \"splat\": {\"manifest\": {\"path\": \"D:/o/splat/scene.ply.manifest.json\", \"sha256\": \"s1\"}, \"count\": 43219}}}";

    #[test]
    fn nested_parts_parse_scoped() {
        let parts = object(MAN, "parts").unwrap();
        assert!(parts.contains("\"mesh\""));
        let mesh = object(parts, "mesh").unwrap();
        let splat = object(parts, "splat").unwrap();
        let mman = object(mesh, "manifest").unwrap();
        let sman = object(splat, "manifest").unwrap();
        assert_eq!(
            field(mman, "path").as_deref(),
            Some("D:/o/mesh/asset.manifest.json")
        );
        assert_eq!(field(mman, "sha256").as_deref(), Some("m1"));
        assert_eq!(field(sman, "sha256").as_deref(), Some("s1"));
        assert_eq!(number(mesh, "levels"), Some(3.0));
        assert_eq!(number(splat, "count"), Some(43219.0));
        // 嵌套对象不能把外层截断（splat 在 mesh 之后仍能读到）
        assert!(parts.find("splat").unwrap() > parts.find("mesh").unwrap());
    }

    #[test]
    fn hash_and_count_checks_are_deciding() {
        assert!(sha_matches("ab", "ab"));
        assert!(!sha_matches("ab", "ac"));
        assert!(!sha_matches("", ""));
        assert!(counts_match(3, 3));
        assert!(!counts_match(3, 4));
        assert!(!counts_match(0, 0), "0 档/0 高斯不是有效资产");
    }

    #[test]
    fn relative_sub_manifest_paths_resolve_against_bundle() {
        let base = Path::new("bundle");
        assert_eq!(
            resolve(base, "mesh/a.json".into()),
            PathBuf::from("bundle/mesh/a.json")
        );
        let abs = std::env::temp_dir().join("trit_scene_abs.json");
        assert_eq!(resolve(base, abs.display().to_string()), abs);
    }

    #[test]
    fn count_ids_handles_nested_arrays() {
        let t = "{\"objects\": [{\"id\": 1, \"bbox_min\": [0,0,0], \"bbox_max\": [1,1,1]}, {\"id\": 2, \"bbox_min\": [1,0,0], \"bbox_max\": [2,1,1]}]}";
        assert_eq!(count_ids(t, "objects"), 2);
        assert_eq!(count_ids("{}", "objects"), 0);
    }

    #[test]
    fn count_entries_counts_only_objects() {
        let t = "{\"levels\": [{\"cell\": 0.01}, {\"cell\": 0.02}], \"other\": [1, 2]}";
        assert_eq!(count_entries(t, "levels"), 2);
        assert_eq!(count_entries(t, "other"), 0);
        assert_eq!(count_entries("{}", "levels"), 0);
    }
}
