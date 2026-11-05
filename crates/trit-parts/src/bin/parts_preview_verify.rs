//! `parts-preview-verify`：新视角预览（A2）的**独立复核**。
//!
//! 重算资产 SHA 与包围盒，用同一环绕相机**逐视角重渲**并与盘上的 PNG 逐像素比对，
//! 再重拼联系表。不一致就写 `preview.verify.reject.json` 并退出码 1。
//!
//! ```text
//! parts-preview-verify --preview runs/preview/scene0084
//! ```

use std::path::{Path, PathBuf};

use trit_parts::data::read_rgb;
use trit_parts::partsrun::{created_unix, git_out, json_str};
use trit_parts::scenefp::norm_root;
use trit_parts::splat::{bounds, contact_sheet, orbit_camera, read_ply_3dgs, render};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!("用法: parts-preview-verify --preview <目录> [--splat <ply>] [--json-out <path>]");
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

struct Frame {
    i: usize,
    yaw: f32,
    path: String,
    sha: String,
}

fn frames(txt: &str) -> Vec<Frame> {
    let mut out = Vec::new();
    for seg in txt.split("{\"i\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        out.push(Frame {
            i: seg
                .split(',')
                .next()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            yaw: number(seg, "yaw").unwrap_or(0.0) as f32,
            path: field(seg, "path").unwrap_or_default(),
            sha: field(seg, "sha256").unwrap_or_default(),
        });
    }
    out
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::INFINITY;
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

fn resolve(base: &Path, p: String) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() { p } else { base.join(p) }
}

fn near3(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= 1e-6)
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

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let dir = arg(&argv, "--preview").map(PathBuf::from);
    let mpath = match dir {
        Some(d) => d.join("preview.manifest.json"),
        None => usage(),
    };
    let base = mpath
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = read(&mpath);
    let cfg = block(&manifest, "config").unwrap_or_else(|| usage());
    let src = block(&manifest, "source").unwrap_or_else(|| usage());
    let bnd = block(&manifest, "bounds").unwrap_or_else(|| usage());
    let sh = block(&manifest, "sheet").unwrap_or_else(|| usage());
    let views = number(cfg, "views").unwrap_or(0.0) as usize;
    let res = number(cfg, "res").unwrap_or(0.0) as usize;
    let elev = number(cfg, "elev").unwrap_or(20.0) as f32;
    let fov = number(cfg, "fov").unwrap_or(40.0) as f32;
    let cols = number(cfg, "cols").unwrap_or(4.0) as usize;
    let bg = number(cfg, "bg").unwrap_or(0.0) as f32;
    let radius = number(cfg, "radius").unwrap_or(0.0) as f32;

    let ply = arg(&argv, "--splat")
        .map(PathBuf::from)
        .unwrap_or_else(|| resolve(&base, field(src, "path").unwrap_or_else(|| usage())));
    let splats = read_ply_3dgs(&ply).unwrap_or_else(|e| {
        eprintln!("读 PLY 失败: {e}");
        std::process::exit(2);
    });
    let ok_src = field(src, "sha256") == Some(hash(&ply))
        && number(src, "count") == Some(splats.len() as f64);
    let (lo, hi) = bounds(&splats);
    let want_lo = nums(bnd, "min");
    let want_hi = nums(bnd, "max");
    let ok_bounds = want_lo.len() == 3
        && want_hi.len() == 3
        && near3(
            [want_lo[0], want_lo[1], want_lo[2]],
            [lo[0] as f64, lo[1] as f64, lo[2] as f64],
        )
        && near3(
            [want_hi[0], want_hi[1], want_hi[2]],
            [hi[0] as f64, hi[1] as f64, hi[2] as f64],
        );
    let center = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);

    let rows = frames(&manifest);
    let mut imgs = Vec::new();
    let mut bad = Vec::new();
    for (k, f) in rows.iter().enumerate() {
        let yaw = 360.0 * k as f32 / views.max(1) as f32;
        let cam = orbit_camera(center, radius, elev, fov, res, yaw);
        let img = render(&splats, &cam, res, res, [bg; 3]);
        let got = read_rgb(&base.join(&f.path)).map(|(_, _, p)| p);
        let diff = got
            .as_ref()
            .map(|g| max_diff(&img, g))
            .unwrap_or(f32::INFINITY);
        let ok = (yaw - f.yaw).abs() < 1e-4 && f.sha == hash(&base.join(&f.path)) && diff <= 0.005;
        println!(
            "视角 {:02}：yaw {:.0}°，像素最大差 {:.4}{}",
            f.i,
            f.yaw,
            diff,
            if ok { "" } else { "  ← 不一致" }
        );
        if !ok {
            bad.push(f.i);
        }
        imgs.push(img);
    }
    let (sw, wh, sheet) = contact_sheet(&imgs, res, cols);
    let got_sheet = read_rgb(&base.join(field(sh, "path").unwrap_or_default()))
        .map(|(_, _, p)| p)
        .unwrap_or_default();
    let sheet_diff = max_diff(&sheet, &got_sheet);
    let ok_sheet = number(sh, "w") == Some(sw as f64)
        && number(sh, "h") == Some(wh as f64)
        && field(sh, "sha256") == Some(hash(&base.join("preview.png")))
        && sheet_diff <= 0.005;
    let ok_frames = !rows.is_empty() && rows.len() == views && bad.is_empty();
    let pass = ok_src && ok_bounds && ok_frames && ok_sheet;
    println!(
        "复核 {}：资产 {}｜包围盒 {}｜视角 {}/{}｜拼图 {}（最大差 {sheet_diff:.4}）",
        mpath.display(),
        if ok_src { "一致" } else { "不一致" },
        if ok_bounds { "一致" } else { "不一致" },
        rows.len() - bad.len(),
        rows.len(),
        if ok_sheet { "一致" } else { "不一致" }
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：资产 / 包围盒 / 逐视角像素 / 拼图全一致"
        } else {
            "复核 FAIL：与清单或重渲结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-preview-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"preview\": {{\"manifest\": {{\"path\": {}, \"sha256\": \"{}\"}}}},\n  \"source\": {{\"path\": {}, \"sha256\": \"{}\", \"count\": {}, \"ok\": {ok_src}}},\n  \"bounds\": {{\"ok\": {ok_bounds}}},\n  \"frames\": {{\"total\": {}, \"bad\": [{}], \"ok\": {ok_frames}}},\n  \"sheet\": {{\"max_diff\": {sheet_diff}, \"ok\": {ok_sheet}}},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&norm_root(&mpath)),
        hash(&mpath),
        json_str(&norm_root(&ply)),
        hash(&ply),
        splats.len(),
        rows.len(),
        bad.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        if pass { "pass" } else { "reject" }
    );
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        base.join(if pass {
            "preview.verify.json"
        } else {
            "preview.verify.reject.json"
        })
    });
    if custom.is_none() {
        let _ = std::fs::remove_file(base.join(if pass {
            "preview.verify.reject.json"
        } else {
            "preview.verify.json"
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

    const MAN: &str = "{\"kind\": \"extract-preview\", \"config\": {\"views\": 2, \"res\": 16, \"elev\": 20, \"fov\": 40, \"cols\": 2, \"bg\": 0, \"radius\": 3}, \"source\": {\"path\": \"s.ply\", \"sha256\": \"ab\", \"count\": 9}, \"bounds\": {\"min\": [-1, 0, -1], \"max\": [1, 2, 1]}, \"frames\": [{\"i\": 0, \"yaw\": 0, \"path\": \"view-00.png\", \"sha256\": \"f0\"}, {\"i\": 1, \"yaw\": 180, \"path\": \"view-01.png\", \"sha256\": \"f1\"}], \"sheet\": {\"path\": \"preview.png\", \"sha256\": \"sh\", \"w\": 32, \"h\": 16}}";

    #[test]
    fn parses_preview_manifest() {
        assert_eq!(number(block(MAN, "config").unwrap(), "views"), Some(2.0));
        assert_eq!(number(block(MAN, "config").unwrap(), "radius"), Some(3.0));
        let src = block(MAN, "source").unwrap();
        assert_eq!(field(src, "sha256").as_deref(), Some("ab"));
        assert_eq!(number(src, "count"), Some(9.0));
        let b = block(MAN, "bounds").unwrap();
        assert_eq!(nums(b, "min"), vec![-1.0, 0.0, -1.0]);
        let rows = frames(MAN);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].yaw, 180.0);
        assert_eq!(rows[1].sha, "f1");
        assert_eq!(number(block(MAN, "sheet").unwrap(), "w"), Some(32.0));
    }

    #[test]
    fn relative_source_resolves_against_bundle() {
        let base = Path::new("bundle/preview");
        assert_eq!(
            resolve(base, "../compose/composed.ply".into()),
            PathBuf::from("bundle/preview/../compose/composed.ply")
        );
        let abs = std::env::temp_dir().join("trit_prev.ply");
        assert_eq!(resolve(base, abs.display().to_string()), abs);
    }

    #[test]
    fn pixel_diff_is_deciding() {
        let a = vec![0.0f32, 0.5, 1.0];
        assert_eq!(max_diff(&a, &a), 0.0);
        let mut b = a.clone();
        b[1] += 0.01;
        assert!((max_diff(&a, &b) - 0.01).abs() < 1e-6);
        assert_eq!(max_diff(&a, &[0.0, 0.5]), f32::INFINITY);
    }
}
