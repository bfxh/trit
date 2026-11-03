//! `parts-extract-verify`：提取族（A2）的**独立复核**。
//!
//! 新进程重算场景语料的内容级指纹与模型 SHA，然后以子进程用 `--load-model` 重跑
//! `parts-3d occ` 的体积 IoU / 闭合率；与 `<模型>.manifest.json` 不一致就写
//! `<模型>.verify.reject.json` 并退出码 1，全一致才写 `<模型>.verify.json`。
//!
//! ```text
//! parts-extract-verify --model runs/extract/n2.occ1
//! ```

use std::path::PathBuf;

use trit_parts::partsrun::{created_unix, git_out, json_str};
use trit_parts::scenefp::{fingerprint_scenes, norm_root};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!(
        "用法: parts-extract-verify --model <occ 模型> [--manifest <json>] \
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

fn jnum(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
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

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let model = arg(&argv, "--model")
        .map(PathBuf::from)
        .unwrap_or_else(|| usage());
    let mut mp = model.as_os_str().to_owned();
    mp.push(".manifest.json");
    let mpath = arg(&argv, "--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(mp));
    let bin = arg(&argv, "--parts-3d")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-3d"));
    let manifest = std::fs::read_to_string(&mpath).unwrap_or_else(|e| {
        eprintln!("读不了清单 {}: {e}", mpath.display());
        std::process::exit(2);
    });

    let renders = PathBuf::from(field(&manifest, "renders").unwrap_or_else(|| usage()));
    let scenes = PathBuf::from(field(&manifest, "scenes_root").unwrap_or_else(|| usage()));
    let want_hash = field(&manifest, "hash").unwrap_or_default();
    let cell = number(&manifest, "cell").unwrap_or(0.01);
    let tau = number(&manifest, "tau").unwrap_or(0.01);
    let seal = number(&manifest, "seal").unwrap_or(0.0) as usize;
    let selfsup = manifest.contains("\"selfsup\": true");
    let frames = list(&manifest, "frames").unwrap_or_default();
    let train = list(&manifest, "train").unwrap_or_default();
    let eval = list(&manifest, "eval").unwrap_or_default();
    let want_iou = number(&manifest, "iou_mean").unwrap_or(f64::NAN);
    let want_closed = number(&manifest, "closed").unwrap_or(-1.0);
    let want_scenes = number(&manifest, "scenes").unwrap_or(-1.0);
    if frames.is_empty() || eval.is_empty() {
        eprintln!("清单缺 frames / eval");
        std::process::exit(2);
    }

    let mut ids: Vec<usize> = train.iter().chain(eval.iter()).copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let fp = fingerprint_scenes(&renders, &scenes, &ids, &frames).unwrap_or_else(|e| {
        eprintln!("语料指纹失败: {e}");
        std::process::exit(2);
    });
    let model_sha = trit_core::fingerprint::file_sha256(&model).unwrap_or_else(|e| {
        eprintln!("模型哈希失败: {e}");
        std::process::exit(2);
    });
    let ok_model = manifest.contains("\"kind\": \"extract-occupancy\"")
        && manifest.contains(&format!("\"sha256\": \"{model_sha}\""));
    let ok_data = manifest.contains("\"algo\": \"sha256-content-v1\"")
        && format!("sha256:{}", fp.hash) == want_hash;

    // 复核不复用训练进程：用 --load-model 在新子进程里重跑评测。
    let recheck = model.with_extension("eval.recheck.json");
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("occ")
        .arg("--root")
        .arg(&renders)
        .arg("--scenes")
        .arg(&scenes)
        .arg("--load-model")
        .arg(&model)
        .arg("--eval")
        .arg(
            eval.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(","),
        )
        .arg("--frames")
        .arg(
            frames
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(","),
        )
        .arg("--cell")
        .arg(format!("{cell}"))
        .arg("--tau")
        .arg(format!("{tau}"))
        .arg("--json-out")
        .arg(&recheck);
    if selfsup {
        cmd.arg("--selfsup");
    }
    if seal > 0 {
        cmd.arg("--seal").arg(seal.to_string());
    }
    let out = cmd.output().unwrap_or_else(|e| {
        eprintln!("跑不了 {}: {e}", bin.display());
        std::process::exit(2);
    });
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        println!("parts-3d occ 退出码 {:?}", out.status.code());
    }
    let got = std::fs::read_to_string(&recheck).unwrap_or_default();
    let iou = number(&got, "iou_mean").unwrap_or(f64::NAN);
    let closed = number(&got, "closed").unwrap_or(-1.0);
    let n = number(&got, "scenes").unwrap_or(-1.0);
    let ok_metrics = out.status.success()
        && want_iou.is_finite()
        && iou.is_finite()
        && (want_iou - iou).abs() <= 1e-4
        && closed == want_closed
        && n == want_scenes;

    let pass = ok_model && ok_data && ok_metrics;
    println!(
        "复核 {}：体积 IoU {iou:.4}（清单 {want_iou:.4}）｜闭合 {closed}/{n}（清单 {want_closed}/{want_scenes}）｜模型 {}｜语料 {}",
        model.display(),
        if ok_model { "一致" } else { "不一致" },
        if ok_data { "一致" } else { "不一致" }
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：模型 / 语料指纹 / 体积 IoU / 闭合率全一致"
        } else {
            "复核 FAIL：与清单或重跑结果不一致"
        }
    );

    let manifest_sha = trit_core::fingerprint::file_sha256(&mpath).unwrap_or_default();
    let recheck_sha = trit_core::fingerprint::file_sha256(&recheck).unwrap_or_default();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"model\": {{\"path\": {}, \"sha256\": \"{model_sha}\"}},\n  \"manifest\": {{\"path\": {}, \"sha256\": \"{manifest_sha}\"}},\n  \"data\": {{\"dirs\": {}, \"files\": {}, \"bytes\": {}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"renders\": {}, \"scenes_root\": {}}},\n  \"metrics\": {{\"scenes\": {n}, \"iou_mean\": {}, \"closed\": {}, \"recheck\": {{\"path\": {}, \"sha256\": \"{recheck_sha}\"}}, \"model_ok\": {ok_model}, \"data_ok\": {ok_data}, \"metrics_ok\": {ok_metrics}}}\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&norm_root(&model)),
        json_str(&norm_root(&mpath)),
        fp.dirs,
        fp.files,
        fp.bytes,
        fp.hash,
        json_str(&norm_root(&renders)),
        json_str(&norm_root(&scenes)),
        jnum(iou),
        jnum(closed),
        json_str(&norm_root(&recheck))
    );
    // 只有写默认位置时才维护「PASS/reject 互斥」；--json-out 是旁路记录，不碰正式记录。
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        let mut s = model.as_os_str().to_owned();
        s.push(if pass {
            ".verify.json"
        } else {
            ".verify.reject.json"
        });
        PathBuf::from(s)
    });
    if custom.is_none() {
        let mut s = model.as_os_str().to_owned();
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

#[cfg(test)]
mod tests {
    use super::*;

    const MAN: &str = "{\"kind\": \"extract-occupancy\", \"config\": {\"cell\": 0.01, \"frames\": [0, 1, 2], \"eval\": [84, 85], \"selfsup\": false}, \"data\": {\"hash\": \"sha256:ab\"}, \"result\": {\"scenes\": 2, \"iou_mean\": 0.554, \"closed\": 2}}";

    #[test]
    fn parses_manifest_fields() {
        assert_eq!(field(MAN, "hash").as_deref(), Some("sha256:ab"));
        assert_eq!(number(MAN, "cell"), Some(0.01));
        assert_eq!(number(MAN, "iou_mean"), Some(0.554));
        assert_eq!(number(MAN, "closed"), Some(2.0));
        assert_eq!(list(MAN, "frames"), Some(vec![0, 1, 2]));
        assert_eq!(list(MAN, "eval"), Some(vec![84, 85]));
        assert_eq!(list(MAN, "nope"), None);
        assert!(!MAN.contains("\"selfsup\": true"));
    }
}
