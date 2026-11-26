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

use trit_learn::json::Json;
use trit_learn::verify::{publish, sibling_exe};
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

fn num(j: &Json, key: &str) -> Option<f64> {
    j.get(key).and_then(Json::as_f64)
}

fn text(j: &Json, key: &str) -> Option<String> {
    j.get_str(key).map(|s| s.to_string())
}

/// 数组里对象条目的数量（levels / objects 这类）。
fn count_entries(j: &Json, key: &str) -> usize {
    j.get(key)
        .and_then(Json::as_array)
        .map(|items| items.iter().filter(|v| matches!(v, Json::Obj(_))).count())
        .unwrap_or(0)
}

fn count_ids(j: &Json, key: &str) -> usize {
    count_entries(j, key)
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

fn hash(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

fn read(p: &Path) -> Json {
    trit_learn::json::read_json(p).unwrap_or_else(|e| {
        trit_log::error!("读不了 {e}");
        std::process::exit(2);
    })
}

fn read_text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn run(bin: &Path, args: &[String], what: &str) -> std::process::Output {
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| {
            trit_log::error!("跑不了 {}: {e}", bin.display());
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
        .unwrap_or_else(|| sibling_exe("parts-asset-verify"));
    let splat_verify = arg(&argv, "--parts-splat-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling_exe("parts-splat-verify"));
    let objects_verify = arg(&argv, "--objects-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling_exe("parts-objects-verify"));
    let compose_verify = arg(&argv, "--compose-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling_exe("parts-compose-verify"));
    let omesh_verify = arg(&argv, "--objectmesh-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling_exe("parts-objectmesh-verify"));
    let preview_verify = arg(&argv, "--preview-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling_exe("parts-preview-verify"));
    let glb_verify = arg(&argv, "--glb-verify")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling_exe("parts-glb-verify"));
    let manifest = read(&mpath);
    let parts = &manifest.get("parts").unwrap_or_else(|| usage());
    let mesh = parts.get("mesh").unwrap_or_else(|| usage());
    let splat = parts.get("splat").unwrap_or_else(|| usage());
    let mman = mesh.get("manifest").unwrap_or_else(|| usage());
    let sman = splat.get("manifest").unwrap_or_else(|| usage());
    let mpath_sub = resolve(&scene_dir, text(mman, "path").unwrap_or_else(|| usage()));
    let spath_sub = resolve(&scene_dir, text(sman, "path").unwrap_or_else(|| usage()));
    let msha = text(mman, "sha256").unwrap_or_default();
    let ssha = text(sman, "sha256").unwrap_or_default();
    let want_levels = num(mesh, "levels").unwrap_or(-1.0) as usize;
    let want_splats = num(splat, "count").unwrap_or(-1.0) as usize;

    let mtxt = read(&mpath_sub);
    let stxt = read(&spath_sub);
    let sblock = stxt.get("splat").unwrap_or_else(|| usage());
    let ply = spath_sub
        .parent()
        .map(|d| d.join(text(sblock, "path").unwrap_or_else(|| usage())))
        .unwrap_or_else(|| usage());
    let ok_man = sha_matches(&msha, &hash(&mpath_sub)) && sha_matches(&ssha, &hash(&spath_sub));
    let got_levels = count_entries(&mtxt, "levels");
    let got_splats = num(&stxt, "count").unwrap_or(-1.0) as usize;
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
        .map(|p| read_text(&p))
        .unwrap_or_default();
    let sverdict = {
        let mut s = ply.as_os_str().to_owned();
        s.push(".verify.json");
        let p = PathBuf::from(s);
        if p.is_file() {
            read_text(&p)
        } else {
            String::new()
        }
    };
    let ok_sub = mout.status.success()
        && sout.status.success()
        && mverdict.contains("\"verdict\": \"pass\"")
        && sverdict.contains("\"verdict\": \"pass\"");

    // `--full` 的四个子包（旧清单没有就跳过）
    let mut extra_ok = true;
    let mut extra = String::new();
    if let Some(ob) = parts.get("objects") {
        let man = ob.get("manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, text(man, "path").unwrap_or_else(|| usage()));
        let d = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| scene_dir.clone());
        let want = num(ob, "count").unwrap_or(-1.0) as usize;
        let got = count_ids(&read(&p), "objects");
        let o = run(
            &objects_verify,
            &["--objects".into(), d.display().to_string()],
            "parts-objects-verify",
        );
        let v = d.join("objects.verify.json");
        let ok = sha_matches(&text(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, got)
            && o.status.success()
            && v.is_file()
            && read_text(&v).contains("\"verdict\": \"pass\"");
        extra_ok &= ok;
        extra.push_str(&format!(" 对象 {got}/{want}="));
        extra.push_str(if ok { "ok" } else { "FAIL" });
    }
    if let Some(cb) = parts.get("compose") {
        let man = cb.get("manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, text(man, "path").unwrap_or_else(|| usage()));
        let d = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| scene_dir.clone());
        let ctxt = read(&p);
        let composed = ctxt
            .get("composed")
            .map(|b| num(b, "count").unwrap_or(-1.0))
            .unwrap_or(-1.0) as usize;
        let cov = ctxt
            .get("gate")
            .map(|b| num(b, "coverage").unwrap_or(f64::NAN))
            .unwrap_or(f64::NAN);
        let want = num(cb, "count").unwrap_or(-1.0) as usize;
        let want_cov = num(cb, "coverage").unwrap_or(f64::NAN);
        let o = run(
            &compose_verify,
            &["--scene".into(), d.display().to_string()],
            "parts-compose-verify",
        );
        let v = d.join("compose.verify.json");
        let ok = sha_matches(&text(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, composed)
            && (want_cov - cov).abs() <= 1e-9
            && o.status.success()
            && v.is_file()
            && read_text(&v).contains("\"verdict\": \"pass\"");
        extra_ok &= ok;
        extra.push_str(&format!(" 合成 {composed}/{want}="));
        extra.push_str(if ok { "ok" } else { "FAIL" });
    }
    if let Some(mb) = parts.get("objects_mesh") {
        let man = mb.get("manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, text(man, "path").unwrap_or_else(|| usage()));
        let d = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| scene_dir.clone());
        let want = num(mb, "count").unwrap_or(-1.0) as usize;
        let got = count_ids(&read(&p), "objects");
        let o = run(
            &omesh_verify,
            &["--dir".into(), d.display().to_string()],
            "parts-objectmesh-verify",
        );
        let v = d.join("objects-mesh.verify.json");
        let ok = sha_matches(&text(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, got)
            && o.status.success()
            && v.is_file()
            && read_text(&v).contains("\"verdict\": \"pass\"");
        extra_ok &= ok;
        extra.push_str(&format!(" 对象网格 {got}/{want}="));
        extra.push_str(if ok { "ok" } else { "FAIL" });
    }
    if let Some(gb) = parts.get("glb") {
        let man = gb.get("manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, text(man, "path").unwrap_or_else(|| usage()));
        let glb_path = p
            .to_string_lossy()
            .strip_suffix(".manifest.json")
            .map(PathBuf::from)
            .unwrap_or_else(|| p.clone());
        let want = num(gb, "parts").unwrap_or(-1.0) as usize;
        let got = read(&p)
            .get("glb")
            .map(|b| num(b, "parts").unwrap_or(-1.0))
            .unwrap_or(-1.0) as usize;
        let o = run(
            &glb_verify,
            &["--glb".into(), glb_path.display().to_string()],
            "parts-glb-verify",
        );
        let mut v = glb_path.as_os_str().to_owned();
        v.push(".verify.json");
        let v = PathBuf::from(v);
        let ok = sha_matches(&text(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, got)
            && o.status.success()
            && v.is_file()
            && read_text(&v).contains("\"verdict\": \"pass\"");
        extra_ok &= ok;
        extra.push_str(&format!(" GLB {got}/{want}="));
        extra.push_str(if ok { "ok" } else { "FAIL" });
    }
    if let Some(pb) = parts.get("preview") {
        let man = pb.get("manifest").unwrap_or_else(|| usage());
        let p = resolve(&scene_dir, text(man, "path").unwrap_or_else(|| usage()));
        let d = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| scene_dir.clone());
        let want = num(pb, "views").unwrap_or(-1.0) as usize;
        let got = read(&p)
            .get("config")
            .map(|c| num(c, "views").unwrap_or(-1.0))
            .unwrap_or(-1.0) as usize;
        let o = run(
            &preview_verify,
            &["--preview".into(), d.display().to_string()],
            "parts-preview-verify",
        );
        let v = d.join("preview.verify.json");
        let ok = sha_matches(&text(man, "sha256").unwrap_or_default(), &hash(&p))
            && counts_match(want, got)
            && o.status.success()
            && v.is_file()
            && read_text(&v).contains("\"verdict\": \"pass\"");
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
    let vp = publish(&scene_dir, "scene", pass, custom.as_deref(), &body).unwrap_or_else(|e| {
        trit_log::error!("{e}");
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
        let m = Json::parse(MAN).unwrap();
        let parts = m.get("parts").unwrap();
        assert!(parts.get("mesh").is_some());
        let mesh = parts.get("mesh").unwrap();
        let splat = parts.get("splat").unwrap();
        let mman = mesh.get("manifest").unwrap();
        let sman = splat.get("manifest").unwrap();
        assert_eq!(mman.get_str("path"), Some("D:/o/mesh/asset.manifest.json"));
        assert_eq!(mman.get_str("sha256"), Some("m1"));
        assert_eq!(sman.get_str("sha256"), Some("s1"));
        assert_eq!(num(mesh, "levels"), Some(3.0));
        assert_eq!(num(splat, "count"), Some(43219.0));
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
        let j = Json::parse(t).unwrap();
        assert_eq!(count_ids(&j, "objects"), 2);
        assert_eq!(count_ids(&Json::parse("{}").unwrap(), "objects"), 0);
    }

    #[test]
    fn count_entries_counts_only_objects() {
        let t = "{\"levels\": [{\"cell\": 0.01}, {\"cell\": 0.02}], \"other\": [1, 2]}";
        let j = Json::parse(t).unwrap();
        assert_eq!(count_entries(&j, "levels"), 2);
        assert_eq!(count_entries(&j, "other"), 0);
        assert_eq!(count_entries(&Json::parse("{}").unwrap(), "levels"), 0);
    }
}
