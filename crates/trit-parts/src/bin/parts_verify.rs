//! `parts-verify`：部件头（A2）的**独立复核**。
//!
//! 新进程、重新从磁盘读数据、重算「训练集 + 评测语料」的内容级指纹与模型 SHA-256，
//! 再以子进程重跑 `parts-inst eval`，把 2D 指标写进 `<tpm>.verify.json`。
//! 与 `<tpm>.manifest.json` 的模型/数据指纹不一致 ⇒ 写 `.verify.reject.json` 并退出码 1。
//!
//! 例：
//! ```text
//! parts-verify --model E:/runs/pb300/pb35k_decay.tpm \
//!   --data E:/trit-data/s1/mat60,E:/trit-data/s1/mat240 --limit 0 \
//!   --renders E:/runs/pb300/holdout/renders --scenes E:/runs/pb300/holdout/scenes \
//!   --eval-args "--gt part --boundary --k 5 --res 384"
//! ```

use std::path::PathBuf;

use trit_parts::partsrun::{
    Fp, created_unix, expand_frames, fingerprint_eval, fingerprint_train, json_str, norm,
};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!(
        "用法: parts-verify --model <tpm> --data <根目录,...> --renders <dir> --scenes <dir> \
         [--limit N] [--seg-sub seg] [--eval-args \"--gt part --boundary --k 5 --res 384\"] \
         [--parts-inst <exe>] [--manifest <json>] [--json-out <path>]"
    );
    std::process::exit(2);
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

/// 取 `pat` 之后紧跟的第一个数字（复数行里只喂最后一行）。
fn num_after(line: &str, pat: &str) -> Option<f64> {
    let i = line.find(pat)?;
    let rest = &line[i + pat.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn fp_json(fp: &Fp) -> String {
    format!(
        "{{\"dirs\": {}, \"files\": {}, \"bytes\": {}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\"}}",
        fp.dirs, fp.files, fp.bytes, fp.hash
    )
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let model = arg(&argv, "--model")
        .map(PathBuf::from)
        .unwrap_or_else(|| usage());
    let data: Vec<PathBuf> = arg(&argv, "--data")
        .unwrap_or_else(|| usage())
        .split(',')
        .map(PathBuf::from)
        .collect();
    let renders = PathBuf::from(arg(&argv, "--renders").unwrap_or_else(|| usage()));
    let scenes = PathBuf::from(arg(&argv, "--scenes").unwrap_or_else(|| usage()));
    let limit: usize = arg(&argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let seg_sub = arg(&argv, "--seg-sub").unwrap_or_else(|| "seg".into());
    let eval_args: Vec<String> = arg(&argv, "--eval-args")
        .unwrap_or_else(|| "--gt part --boundary --k 5 --res 384".into())
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    let eval_limit: usize = eval_args
        .iter()
        .position(|a| a == "--limit")
        .and_then(|i| eval_args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let inst_bin = arg(&argv, "--parts-inst")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling("parts-inst"));

    // 训练集：与 parts-train 同一条展开规则 + 同一个 limit。
    let frames = expand_frames(&data, limit);
    if frames.is_empty() {
        eprintln!("训练集为空：检查 --data / --limit");
        std::process::exit(2);
    }
    let train_fp = fingerprint_train(&frames, &seg_sub).unwrap_or_else(|e| {
        eprintln!("训练集指纹失败: {e}");
        std::process::exit(2);
    });
    let eval_fp = fingerprint_eval(&renders, &scenes, eval_limit).unwrap_or_else(|e| {
        eprintln!("评测语料指纹失败: {e}");
        std::process::exit(2);
    });
    let model_sha = trit_parts::fingerprint::file_sha256(&model).unwrap_or_else(|e| {
        eprintln!("模型哈希失败: {e}");
        std::process::exit(2);
    });

    // 复核器不复用训练进程的任何状态：评测由 parts-inst 在新的子进程里跑。
    let mut cmd = std::process::Command::new(&inst_bin);
    cmd.arg("eval")
        .arg("--model")
        .arg(&model)
        .arg("--renders")
        .arg(&renders)
        .arg("--scenes")
        .arg(&scenes)
        .args(&eval_args);
    let out = cmd.output().unwrap_or_else(|e| {
        eprintln!("跑不了 {}: {e}", inst_bin.display());
        std::process::exit(2);
    });
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprint!("{stderr}");
    let last = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string();
    println!("{last}");

    let m = |k: &str| num_after(&last, k);
    let metrics = format!(
        "{{\"frames\": {}, \"miou\": {}, \"miou_median\": {}, \"fg_iou\": {}, \"pred_gt\": {}, \"pred_gt_p90\": {}, \"count_pm1\": {}, \"count_exact\": {}, \"bnd_p\": {}, \"bnd_r\": {}, \"bnd_f1\": {}, \"mixed_pct\": {}}}",
        m("frames=").map(|v| v as u64).unwrap_or(0),
        m("mIoU mean ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("median ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("前景 IoU mean ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("pred/gt ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("p90 ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("计数±1 ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("计数全对 ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("边界 P ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m(" R ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("F1 ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        m("跨物体混合 ")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
    );

    let mut mp = model.as_os_str().to_owned();
    mp.push(".manifest.json");
    let mpath = arg(&argv, "--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(mp));
    let manifest = if mpath.exists() {
        Some(std::fs::read_to_string(&mpath).unwrap_or_default())
    } else {
        None
    };
    let (mut ok_model, mut ok_data, mut note) = (true, true, "没有清单，只写复核记录");
    if let Some(txt) = &manifest {
        ok_model = txt.contains("\"kind\": \"parts-train\"")
            && txt.contains(&format!("\"sha256\": \"{model_sha}\""));
        ok_data = txt.contains("\"algo\": \"sha256-content-v1\"")
            && txt.contains(&format!("\"hash\": \"sha256:{}\"", train_fp.hash));
        note = if ok_model && ok_data {
            "复核 PASS：模型与训练集指纹与清单一致"
        } else {
            "复核 FAIL：与清单不一致"
        };
    }
    let verdict = if ok_model && ok_data && out.status.success() {
        "pass"
    } else {
        "reject"
    };
    if !out.status.success() {
        eprintln!("parts-inst eval 退出码 {:?}", out.status.code());
    }
    if verdict == "pass" {
        println!("{note}");
    } else {
        eprintln!("{note}（模型={ok_model} 训练集={ok_data}）");
    }

    let manifest_sha = if mpath.exists() {
        format!(
            "\"{}\"",
            trit_parts::fingerprint::file_sha256(&mpath).unwrap_or_default()
        )
    } else {
        "null".into()
    };
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"parts-holdout-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"model\": {{\"path\": {}, \"sha256\": \"{model_sha}\"}},\n  \"manifest\": {{\"path\": {}, \"sha256\": {manifest_sha}}},\n  \"data\": {},\n  \"eval\": {{\"renders\": {}, \"scenes\": {}, \"args\": [{}], \"data\": {}}},\n  \"metrics\": {metrics}\n}}\n",
        created_unix(),
        json_str(trit_parts::partsrun::git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&norm(&model)),
        json_str(&norm(&mpath)),
        fp_json(&train_fp),
        json_str(&norm(&renders)),
        json_str(&norm(&scenes)),
        eval_args
            .iter()
            .map(|a| json_str(a))
            .collect::<Vec<_>>()
            .join(", "),
        fp_json(&eval_fp)
    );
    let vp = match arg(&argv, "--json-out") {
        Some(p) => PathBuf::from(p),
        None => {
            let mut s = model.as_os_str().to_owned();
            s.push(if verdict == "pass" {
                ".verify.json"
            } else {
                ".verify.reject.json"
            });
            PathBuf::from(s)
        }
    };
    std::fs::write(&vp, body).unwrap_or_else(|e| {
        eprintln!("写复核记录失败 {}: {e}", vp.display());
        std::process::exit(2);
    });
    println!("复核记录 {}（{verdict}）", vp.display());
    if verdict != "pass" {
        std::process::exit(1);
    }
}
