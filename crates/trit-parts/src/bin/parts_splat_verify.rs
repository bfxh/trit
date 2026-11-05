//! `parts-splat-verify`：高斯资产（A2）的**独立复核**。
//!
//! 新进程重算场景语料指纹与 PLY 的 SHA，把盘上的 PLY **反解回高斯**，再用同一批评测视角
//! 重渲并重算前景 PSNR；与 `<ply>.manifest.json` 不一致就写 `<ply>.verify.reject.json`
//! 并退出码 1，全一致才写 `<ply>.verify.json`。
//!
//! ```text
//! parts-splat-verify --splat runs/splat/scene0084.ply
//! ```

use std::path::{Path, PathBuf};

use trit_parts::data::{read_npy_i32, read_rgb};
use trit_parts::fuse::View;
use trit_parts::geo::{Camera, read_npy_f32};
use trit_parts::partsrun::{created_unix, git_out, json_str};
use trit_parts::scenefp::{fingerprint_scene_frames_rgb, norm_root};
use trit_parts::splat::{masked_psnr, read_ply_3dgs};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!("用法: parts-splat-verify --splat <ply> [--manifest <json>] [--json-out <path>]");
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

fn list(txt: &str, key: &str) -> Option<Vec<usize>> {
    let pat = format!("\"{key}\": [");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest.find(']')?;
    Some(
        rest[..end]
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect(),
    )
}

/// 取 `"key": { ... }` 的对象体（清单里的对象都不嵌套）。
fn block<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": {{");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    Some(&rest[..rest.find('}')?])
}

fn psnr_matches(want: f64, got: f64) -> bool {
    want.is_finite() && got.is_finite() && (want - got).abs() <= 1e-3
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

fn mean_psnr(splats: &[trit_parts::splat::Splat], scene: &Path, frames: &[usize]) -> f64 {
    let mut out = Vec::new();
    for &f in frames {
        let v = load_view(scene, f);
        let gt = load_rgb(scene, f);
        let img = trit_parts::splat::render(splats, &v.cam, v.w, v.h, [0.0, 0.0, 0.0]);
        out.push(masked_psnr(&img, &gt, &v.ids));
    }
    let f: Vec<f64> = out.into_iter().filter(|x| x.is_finite()).collect();
    f.iter().sum::<f64>() / f.len().max(1) as f64
}

fn read_manifest(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| {
        eprintln!("读不了清单 {}: {e}", p.display());
        std::process::exit(2);
    })
}

fn hash(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

fn jnum(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.6}")
    } else {
        "null".into()
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let ply = PathBuf::from(arg(&argv, "--splat").unwrap_or_else(|| usage()));
    let mut mp = ply.as_os_str().to_owned();
    mp.push(".manifest.json");
    let mpath = arg(&argv, "--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(mp));
    let manifest = read_manifest(&mpath);

    let data = block(&manifest, "data").unwrap_or_else(|| usage());
    let splat = block(&manifest, "splat").unwrap_or_else(|| usage());
    let metrics = block(&manifest, "metrics").unwrap_or_else(|| usage());
    let scene = PathBuf::from(field(data, "scene").unwrap_or_else(|| usage()));
    let fit_frames = list(&manifest, "fit_frames").unwrap_or_default();
    let eval_frames = list(&manifest, "eval_frames").unwrap_or_default();
    if fit_frames.is_empty() || eval_frames.is_empty() {
        eprintln!("清单缺 fit_frames / eval_frames");
        std::process::exit(2);
    }
    let want_hash = field(data, "hash").unwrap_or_default();
    let want_files = number(data, "files").unwrap_or(-1.0);
    let want_bytes = number(data, "bytes").unwrap_or(-1.0);
    let want_sha = field(splat, "sha256").unwrap_or_default();
    let want_bytes_file = number(splat, "bytes").unwrap_or(-1.0);
    let want_count = number(splat, "count").unwrap_or(-1.0);
    let want_fit = number(metrics, "fit_psnr_after").unwrap_or(f64::NAN);
    let want_eval = number(metrics, "eval_psnr_after").unwrap_or(f64::NAN);

    let mut all: Vec<usize> = fit_frames.iter().chain(&eval_frames).copied().collect();
    all.sort_unstable();
    all.dedup();
    let fp = fingerprint_scene_frames_rgb(&scene, &all).unwrap_or_else(|e| {
        eprintln!("语料指纹失败: {e}");
        std::process::exit(2);
    });
    let ok_data = manifest.contains("\"algo\": \"sha256-content-v1\"")
        && want_hash == format!("sha256:{}", fp.hash)
        && want_files == fp.files as f64
        && want_bytes == fp.bytes as f64;

    let file_sha = hash(&ply);
    let file_bytes = std::fs::metadata(&ply).map(|m| m.len()).unwrap_or(0) as f64;
    let ok_file = manifest.contains("\"kind\": \"extract-splat\"")
        && want_sha == file_sha
        && (want_bytes_file < 0.0 || want_bytes_file == file_bytes);
    let splats = read_ply_3dgs(&ply).unwrap_or_else(|e| {
        eprintln!("读 PLY 失败: {e}");
        std::process::exit(2);
    });
    let ok_count = want_count == splats.len() as f64 && !splats.is_empty();

    let fit_got = mean_psnr(&splats, &scene, &fit_frames);
    let eval_got = mean_psnr(&splats, &scene, &eval_frames);
    let ok_psnr = psnr_matches(want_fit, fit_got) && psnr_matches(want_eval, eval_got);

    let pass = ok_data && ok_file && ok_count && ok_psnr;
    println!(
        "复核 {}：文件 {}｜语料 {}/{} 字节 {}｜高斯 {}/{}｜拟合 PSNR {fit_got:.4}（清单 {want_fit:.4}）｜留出 {eval_got:.4}（清单 {want_eval:.4}）",
        mpath.display(),
        if ok_file { "一致" } else { "不一致" },
        fp.files,
        want_files as u64,
        fp.bytes,
        splats.len(),
        want_count as u64,
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：PLY 哈希 / 语料指纹 / 高斯数 / 两批 PSNR 全一致"
        } else {
            "复核 FAIL：与清单或重渲结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-splat-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"splat\": {{\"path\": {}, \"sha256\": \"{file_sha}\", \"bytes\": {file_bytes}, \"count\": {}, \"ok\": {ok_file}}},\n  \"data\": {{\"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"files\": {}, \"bytes\": {}, \"scene\": {}, \"ok\": {ok_data}}},\n  \"metrics\": {{\"fit_psnr\": {}, \"eval_psnr\": {}, \"count_ok\": {ok_count}, \"psnr_ok\": {ok_psnr}}},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&norm_root(&ply)),
        splats.len(),
        fp.hash,
        fp.files,
        fp.bytes,
        json_str(&norm_root(&scene)),
        jnum(fit_got),
        jnum(eval_got),
        if pass { "pass" } else { "reject" }
    );
    // 只有写默认位置时才维护 PASS/reject 互斥；--json-out 是旁路记录。
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        let mut s = ply.as_os_str().to_owned();
        s.push(if pass {
            ".verify.json"
        } else {
            ".verify.reject.json"
        });
        PathBuf::from(s)
    });
    if custom.is_none() {
        let mut s = ply.as_os_str().to_owned();
        s.push(if pass {
            ".verify.reject.json"
        } else {
            ".verify.json"
        });
        let _ = std::fs::remove_file(PathBuf::from(s));
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

fn fail(msg: &str) -> ! {
    eprintln!("FAIL {msg}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_parts::splat::psnr;

    const MAN: &str = "{\"kind\": \"extract-splat\", \"config\": {\"fit_frames\": [0, 2], \"eval_frames\": [1, 3], \"cell\": 0.01}, \"data\": {\"files\": 16, \"bytes\": 900, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:ab\", \"scene\": \"D:/r/scene0084\"}, \"splat\": {\"path\": \"s.ply\", \"sha256\": \"cd\", \"bytes\": 100, \"count\": 7}, \"metrics\": {\"fit_psnr_after\": 26.5, \"eval_psnr_after\": 24.5}}";

    #[test]
    fn parses_manifest_fields() {
        let data = block(MAN, "data").unwrap();
        let splat = block(MAN, "splat").unwrap();
        let metrics = block(MAN, "metrics").unwrap();
        assert_eq!(field(data, "scene").as_deref(), Some("D:/r/scene0084"));
        assert_eq!(field(data, "hash").as_deref(), Some("sha256:ab"));
        assert_eq!(field(splat, "sha256").as_deref(), Some("cd"));
        assert_eq!(number(data, "bytes"), Some(900.0));
        assert_eq!(
            number(splat, "bytes"),
            Some(100.0),
            "data/splat 的 bytes 必须分开读"
        );
        assert_eq!(number(splat, "count"), Some(7.0));
        assert_eq!(number(metrics, "fit_psnr_after"), Some(26.5));
        assert_eq!(list(MAN, "fit_frames"), Some(vec![0, 2]));
        assert_eq!(list(MAN, "eval_frames"), Some(vec![1, 3]));
        assert_eq!(list(MAN, "nope"), None);
        assert!(MAN.contains("\"kind\": \"extract-splat\""));
    }

    #[test]
    fn psnr_tolerance_is_tight_and_finite_only() {
        assert!(psnr_matches(24.5, 24.5004));
        assert!(!psnr_matches(24.5, 24.6), "0.1 dB 的漂移必须判红");
        assert!(!psnr_matches(f64::NAN, 24.5));
        assert!(!psnr_matches(24.5, f64::NAN));
    }

    #[test]
    fn psnr_helper_matches_library() {
        let a = [0.0f32, 0.5, 1.0];
        assert!(psnr(&a, &a).is_infinite());
        let b = [0.1f32, 0.5, 0.9];
        let v = psnr(&a, &b);
        assert!(v.is_finite() && v > 0.0);
    }
}
