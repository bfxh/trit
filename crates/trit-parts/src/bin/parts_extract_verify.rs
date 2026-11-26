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

use trit_learn::json::Json;
use trit_learn::verify::{publish_base, sibling_exe};
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

fn jnum(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

fn list(j: &Json, key: &str) -> Vec<usize> {
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
        .unwrap_or_else(|| sibling_exe("parts-3d"));
    let manifest = trit_learn::json::read_json(&mpath).unwrap_or_else(|e| {
        trit_log::error!("读不了清单 {e}");
        std::process::exit(2);
    });
    let config = manifest.get("config").unwrap_or_else(|| usage());
    let data = manifest.get("data").unwrap_or_else(|| usage());
    let result = manifest.get("result").unwrap_or_else(|| usage());
    let renders = PathBuf::from(data.get_str("renders").unwrap_or_else(|| usage()));
    let scenes = PathBuf::from(data.get_str("scenes_root").unwrap_or_else(|| usage()));
    let want_hash = data.get_str("hash").unwrap_or_default();
    let cell = config.get("cell").and_then(Json::as_f64).unwrap_or(0.01);
    let tau = config.get("tau").and_then(Json::as_f64).unwrap_or(0.01);
    let seal = config.get("seal").and_then(Json::as_f64).unwrap_or(0.0) as usize;
    let selfsup = config
        .get("selfsup")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let frames = list(config, "frames");
    let train = list(config, "train");
    let eval = list(config, "eval");
    let want_iou = result
        .get("iou_mean")
        .and_then(Json::as_f64)
        .unwrap_or(f64::NAN);
    let want_closed = result.get("closed").and_then(Json::as_f64).unwrap_or(-1.0);
    let want_scenes = result.get("scenes").and_then(Json::as_f64).unwrap_or(-1.0);
    if frames.is_empty() || eval.is_empty() {
        trit_log::error!("清单缺 frames / eval");
        std::process::exit(2);
    }

    let mut ids: Vec<usize> = train.iter().chain(eval.iter()).copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let fp = fingerprint_scenes(&renders, &scenes, &ids, &frames).unwrap_or_else(|e| {
        trit_log::error!("语料指纹失败: {e}");
        std::process::exit(2);
    });
    let model_sha = trit_core::fingerprint::file_sha256(&model).unwrap_or_else(|e| {
        trit_log::error!("模型哈希失败: {e}");
        std::process::exit(2);
    });
    let ok_model = manifest.get_str("kind") == Some("extract-occupancy")
        && manifest.get("model").and_then(|m| m.get_str("sha256")) == Some(model_sha.as_str());
    let ok_data = data.get_str("algo") == Some("sha256-content-v1")
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
        trit_log::error!("跑不了 {}: {e}", bin.display());
        std::process::exit(2);
    });
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        println!("parts-3d occ 退出码 {:?}", out.status.code());
    }
    let got = trit_learn::json::read_json(&recheck).unwrap_or(Json::Null);
    let iou = got
        .get("iou_mean")
        .and_then(Json::as_f64)
        .unwrap_or(f64::NAN);
    let closed = got.get("closed").and_then(Json::as_f64).unwrap_or(-1.0);
    let n = got.get("scenes").and_then(Json::as_f64).unwrap_or(-1.0);
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
    let vp = publish_base(&model, pass, custom.as_deref(), &body).unwrap_or_else(|e| {
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

    const MAN: &str = "{\"kind\": \"extract-occupancy\", \"config\": {\"cell\": 0.01, \"frames\": [0, 1, 2], \"eval\": [84, 85], \"selfsup\": false}, \"data\": {\"hash\": \"sha256:ab\"}, \"result\": {\"scenes\": 2, \"iou_mean\": 0.554, \"closed\": 2}}";

    #[test]
    fn parses_manifest_fields() {
        let m = Json::parse(MAN).unwrap();
        let data = m.get("data").unwrap();
        let config = m.get("config").unwrap();
        let result = m.get("result").unwrap();
        assert_eq!(data.get_str("hash"), Some("sha256:ab"));
        assert_eq!(config.get("cell").unwrap().as_f64(), Some(0.01));
        assert_eq!(result.get("iou_mean").unwrap().as_f64(), Some(0.554));
        assert_eq!(result.get("closed").unwrap().as_f64(), Some(2.0));
        assert_eq!(list(config, "frames"), vec![0, 1, 2]);
        assert_eq!(list(config, "eval"), vec![84, 85]);
        assert!(list(config, "nope").is_empty());
        assert_eq!(config.get("selfsup").unwrap().as_bool(), Some(false));
    }
}
