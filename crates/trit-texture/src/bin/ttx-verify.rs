//! `ttx-verify`：纹理族 A2 —— 独立复核一个 TTX 资产包。
//!
//! ```text
//! ttx-verify --pack <产包目录> [--manifest <json>] [--json-out <path>]
//! ```
//!
//! 新进程只信磁盘：重算语料内容级指纹与索引 SHA、逐图比对 TTX 哈希/PSNR/最大误差、
//! 重编码后逐字节比对、重查采样一致性。任一不符写 `pack.verify.reject.json` 并退出码 1；
//! 全过写 `pack.verify.json`。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trit_texture::pack::{encode, fingerprint_pngs, list_pngs, measure, parse_tsv};
use trit_texture::pngio::load_png_rgba;

const USAGE: &str = "用法：ttx-verify --pack <产包目录> [--manifest <json>] [--json-out <path>]";

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
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

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn jnum(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

/// 清单里的字符串字段（本清单由 `ttx-pack` 写出，键名唯一）。
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

struct Tally {
    psnr_mismatch: usize,
    sha_mismatch: usize,
    reencode_mismatch: usize,
    sampling_bad: usize,
    checked: usize,
    files: usize,
    psnr_sum: f64,
    psnr_n: usize,
    bpp_sum: f64,
    max_err: u32,
    /// 读/解码失败的文件数（坏包也要落 reject 记录，不能只打一行错就退出）。
    read_errors: usize,
}

fn run() -> Result<i32, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let pack = PathBuf::from(arg(&argv, "--pack").ok_or(USAGE)?);
    let manifest_path = arg(&argv, "--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| pack.join("pack.manifest.json"));
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读不了清单 {}: {e}", manifest_path.display()))?;
    let root = PathBuf::from(field(&manifest, "root").ok_or("清单缺 data.root")?);
    let want_hash = field(&manifest, "hash").ok_or("清单缺 data.hash")?;
    let want_index = field(&manifest, "index_sha256").ok_or("清单缺 index_sha256")?;
    let limit = number(&manifest, "limit").unwrap_or(0.0) as usize;
    let stride = (number(&manifest, "sample_stride").unwrap_or(5.0) as usize).max(1);
    let mip = manifest.contains("\"mip\": true");
    let want_psnr = number(&manifest, "psnr_mean");

    let mut files = list_pngs(&root)?;
    if limit > 0 {
        files.truncate(limit);
    }
    let fp = fingerprint_pngs(&files)?;
    let ok_data = manifest.contains("\"algo\": \"sha256-content-v1\"")
        && format!("sha256:{}", fp.hash) == want_hash;

    let index_text = std::fs::read_to_string(pack.join("pack.index.tsv"))
        .map_err(|e| format!("读不了索引: {e}"))?;
    let index_sha = trit_core::fingerprint::sha256_hex(index_text.as_bytes());
    let ok_index = index_sha == want_index;
    let entries = parse_tsv(&index_text)?;

    let mut t = Tally {
        psnr_mismatch: 0,
        sha_mismatch: 0,
        reencode_mismatch: 0,
        sampling_bad: 0,
        checked: 0,
        files: 0,
        psnr_sum: 0.0,
        psnr_n: 0,
        bpp_sum: 0.0,
        max_err: 0,
        read_errors: 0,
    };
    for e in &entries {
        let ttx_path = pack.join(format!("{}.ttx", e.name.trim_end_matches(".png")));
        let ttx = match std::fs::read(&ttx_path) {
            Ok(v) => v,
            Err(e2) => {
                eprintln!("读不了 {}: {e2}", ttx_path.display());
                t.read_errors += 1;
                continue;
            }
        };
        let ttx_sha = trit_core::fingerprint::sha256_hex(&ttx);
        if ttx_sha != e.sha256 || ttx.len() as u64 != e.bytes {
            t.sha_mismatch += 1;
        }
        let img = match load_png_rgba(&root.join(&e.name)) {
            Ok(v) => v,
            Err(e2) => {
                eprintln!("{}: {e2}", e.name);
                t.read_errors += 1;
                continue;
            }
        };
        let (psnr, max_err, checked, bad) =
            match measure(&img.rgba, img.width, img.height, &ttx, stride) {
                Ok(v) => v,
                Err(e2) => {
                    eprintln!("{}: {e2}", e.name);
                    t.read_errors += 1;
                    continue;
                }
            };
        t.checked += checked;
        t.sampling_bad += bad;
        t.max_err = t.max_err.max(max_err);
        match (psnr, e.psnr) {
            (None, None) => {}
            (Some(a), Some(b)) if (a - b).abs() <= 1e-6 => {}
            _ => t.psnr_mismatch += 1,
        }
        if let Some(v) = psnr {
            t.psnr_sum += v;
            t.psnr_n += 1;
        }
        t.bpp_sum += e.bpp;
        match encode(&img.rgba, img.width, img.height, mip) {
            Ok(v) if v == ttx => {}
            Ok(_) => t.reencode_mismatch += 1,
            Err(e2) => {
                eprintln!("重编码 {}: {e2}", e.name);
                t.read_errors += 1;
            }
        }
        t.files += 1;
    }

    let psnr_mean = if t.psnr_n == 0 {
        f64::NAN
    } else {
        t.psnr_sum / t.psnr_n as f64
    };
    let bpp_mean = if t.files == 0 {
        f64::NAN
    } else {
        t.bpp_sum / t.files as f64
    };
    let ok_psnr = match (want_psnr, psnr_mean.is_finite()) {
        (Some(w), true) => (w - psnr_mean).abs() <= 1e-6,
        (None, false) => true,
        _ => false,
    };
    let pass = ok_data
        && ok_index
        && t.sha_mismatch == 0
        && t.psnr_mismatch == 0
        && t.reencode_mismatch == 0
        && t.sampling_bad == 0
        && t.read_errors == 0
        && ok_psnr;

    println!(
        "复核 {}：{} 张，{:.4} dB，{:.4} bpp；数据指纹 {}｜索引 {}｜SHA 不符 {}｜PSNR 不符 {}｜重编码不符 {}｜采样 {}/{} 不符",
        pack.display(),
        t.files,
        psnr_mean,
        bpp_mean,
        if ok_data { "一致" } else { "不一致" },
        if ok_index { "一致" } else { "不一致" },
        t.sha_mismatch,
        t.psnr_mismatch,
        t.reencode_mismatch,
        t.sampling_bad,
        t.checked
    );
    if t.read_errors > 0 {
        eprintln!("读/解码失败 {} 个文件", t.read_errors);
    }
    println!(
        "{}",
        if pass {
            "复核 PASS：清单 / 语料 / 逐图指标 / 重编码 / 采样一致性全一致"
        } else {
            "复核 FAIL：与清单或磁盘不一致"
        }
    );

    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let manifest_sha = trit_core::fingerprint::file_sha256(&manifest_path).unwrap_or_default();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"texture-pack-verify\",\n  \"created_unix\": {created},\n  \"commit\": {},\n  \"manifest\": {{\"path\": {}, \"sha256\": \"{manifest_sha}\"}},\n  \"artifact\": {{\"dir\": {}}},\n  \"data\": {{\"files\": {}, \"bytes\": {}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"root\": {}}},\n  \"metrics\": {{\"files\": {}, \"psnr_mean\": {}, \"bpp_mean\": {}, \"max_err\": {}, \"sampling_checked\": {}, \"sampling_bad\": {}, \"sha_mismatch\": {}, \"psnr_mismatch\": {}, \"reencode_mismatch\": {}, \"read_errors\": {}, \"data_ok\": {ok_data}, \"index_ok\": {ok_index}}}\n}}\n",
        jstr(&commit),
        jstr(&norm(&manifest_path)),
        jstr(&norm(&pack)),
        fp.files,
        fp.bytes,
        fp.hash,
        jstr(&norm(&root)),
        t.files,
        jnum(psnr_mean),
        jnum(bpp_mean),
        t.max_err,
        t.checked,
        t.sampling_bad,
        t.sha_mismatch,
        t.psnr_mismatch,
        t.reencode_mismatch,
        t.read_errors
    );
    // 只有写默认位置时才维护「PASS/reject 互斥」；--json-out 是旁路记录，不碰正式记录。
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        pack.join(if pass {
            "pack.verify.json"
        } else {
            "pack.verify.reject.json"
        })
    });
    if custom.is_none() {
        let _ = std::fs::remove_file(pack.join(if pass {
            "pack.verify.reject.json"
        } else {
            "pack.verify.json"
        }));
    }
    std::fs::write(&vp, body).map_err(|e| format!("写复核记录 {}: {e}", vp.display()))?;
    println!(
        "复核记录 {}（{}）",
        vp.display(),
        if pass { "pass" } else { "reject" }
    );
    Ok(if pass { 0 } else { 1 })
}

fn main() -> ExitCode {
    match run() {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("ttx-verify：{e}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
