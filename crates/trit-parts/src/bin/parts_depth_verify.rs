//! `parts-depth-verify`：留出集的**独立复核**（Comparator 式）。
//!
//! 新进程、重新从磁盘读数据、重算模型与数据指纹，在留出集上评一遍，并写
//! `<dcn>.verify.json`。若同目录有 `<dcn>.manifest.json`，还会比对模型/数据指纹：
//! 不一致就判红（防止「训练用的数据/权重和发布的那份不是一回事」）。
//!
//! 例：
//! ```text
//! parts-depth-verify --dcn runs/depth/h128.dcn --data E:/trit-data/s1/mat60/renders/... \
//!   --holdout scene0050,scene0051 --size 64
//! ```

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use trit_parts::depthdata::{EvalSample, data_fingerprint, eval_holdout, is_held, load_sample};
use trit_parts::depthnet::DepthNet;

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn flag(argv: &[String], k: &str) -> bool {
    argv.iter().any(|a| a == k)
}

fn usage() -> ! {
    eprintln!(
        "用法: parts-depth-verify --dcn <model.dcn> --data <dir,...> --holdout a,b \
         [--size 64] [--stride 2] [--pyr 0] [--full]"
    );
    std::process::exit(2);
}

fn git_out(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn jstr(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let dcn = arg(&argv, "--dcn")
        .map(PathBuf::from)
        .unwrap_or_else(|| usage());
    let data: Vec<PathBuf> = arg(&argv, "--data")
        .unwrap_or_else(|| usage())
        .split(',')
        .map(PathBuf::from)
        .collect();
    let holdout: Vec<String> = arg(&argv, "--holdout")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_else(|| usage());
    let size: usize = arg(&argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let stride: usize = arg(&argv, "--stride")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let full = flag(&argv, "--full");

    let net = DepthNet::load(&dcn).unwrap_or_else(|e| panic!("{e}"));
    // 输入构成以**模型里记的**为准（DCN4 的 pyr/ppyr），不接受命令行覆盖：
    // 否则复核跑的可能是另一个输入语义。
    let (pyr, ppyr) = (net.pyr, net.ppyr);
    let mut es: Vec<EvalSample> = Vec::new();
    for d in &data {
        if !is_held(d, &holdout) {
            continue;
        }
        for fr in 0..=24 {
            if fr % stride.max(1) != 0 {
                continue;
            }
            if let Some((x, y, mask, _m, _q, _w)) = load_sample(d, fr, size, full, pyr, ppyr) {
                es.push(EvalSample { x, y, mask });
            }
        }
    }
    if es.is_empty() {
        panic!("留出集为空：检查 --holdout 与 --data（必须与训练时同一份列表）");
    }

    let r = eval_holdout(&net, &es, size);
    let model_sha = trit_parts::fingerprint::file_sha256(&dcn).unwrap_or_default();
    let (dirs, files, bytes, data_hash) = data_fingerprint(&data, stride);
    let commit = git_out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    println!(
        "复核 {}：{} 帧 / {} 像素，形状对齐 log MAE {:.4}、深度相对误差 {:.2}%（常数基线 {:.2}%），δ<1.25 {:.3}；默认尺度 {:.2}%",
        dcn.display(),
        r.frames,
        r.pixels,
        r.mae_log,
        r.rel_pct,
        r.base_pct,
        r.delta,
        r.deploy_pct
    );
    println!(
        "模型 sha256 {model_sha}｜数据 sha256 {data_hash}（{dirs} 目录 / {files} 文件 / {bytes} 字节，内容级）"
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"depth-holdout-verify\",\n  \"created_unix\": {created},\n  \"commit\": {},\n  \"model\": {{\"path\": {}, \"sha256\": \"{model_sha}\"}},\n  \"data\": {{\"dirs\": {dirs}, \"files\": {files}, \"bytes\": {bytes}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{data_hash}\"}},\n  \"holdout\": [{}],\n  \"size\": {size},\n  \"metrics\": {{\"frames\": {}, \"pixels\": {}, \"mae_log\": {}, \"rel_pct\": {}, \"base_pct\": {}, \"delta\": {}, \"deploy_pct\": {}}}\n}}\n",
        jstr(&commit),
        jstr(&dcn.to_string_lossy()),
        holdout
            .iter()
            .map(|h| jstr(h))
            .collect::<Vec<_>>()
            .join(", "),
        r.frames,
        r.pixels,
        r.mae_log,
        r.rel_pct,
        r.base_pct,
        r.delta,
        r.deploy_pct
    );
    let mut mp = dcn.as_os_str().to_owned();
    mp.push(".manifest.json");
    let mp = PathBuf::from(mp);
    // 只有 PASS 才写 `<dcn>.verify.json`（目录只收可信记录）；FAIL 落 `.verify.reject.json` 留证。
    let verdict = if mp.exists() {
        let m = std::fs::read_to_string(&mp).unwrap_or_default();
        let ok_model = m.contains(&format!("\"sha256\": \"{model_sha}\""));
        // 只认内容级算法标记：旧清单（名+大小）与新指纹不可比，必须重训/重写清单。
        let ok_data = m.contains("\"algo\": \"sha256-content-v1\"")
            && m.contains(&format!("\"hash\": \"sha256:{data_hash}\""));
        if ok_model && ok_data {
            println!("复核 PASS：模型与数据指纹与清单一致");
            "pass"
        } else {
            eprintln!("复核 FAIL：模型={ok_model} 数据={ok_data}（与清单不一致）");
            "reject"
        }
    } else {
        println!("没有清单（{}），只写复核记录", mp.display());
        "pass"
    };
    let mut vp = dcn.as_os_str().to_owned();
    vp.push(if verdict == "pass" {
        ".verify.json"
    } else {
        ".verify.reject.json"
    });
    let vp = PathBuf::from(vp);
    std::fs::write(&vp, body).unwrap_or_else(|e| panic!("{}: {e}", vp.display()));
    println!("复核记录 {}（{verdict}）", vp.display());
    if verdict != "pass" {
        std::process::exit(1);
    }
}
