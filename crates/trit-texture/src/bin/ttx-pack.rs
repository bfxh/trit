//! `ttx-pack`：纹理族 A1 —— PNG 语料 → TTX2 资产包 + 机器可读清单。
//!
//! ```text
//! ttx-pack --data <png目录> --out <产包目录> [--limit N] [--mip] [--stride 5]
//!          [--run-manifest <json>]
//! ```
//!
//! 产物：`<out>/<name>.ttx`（每图一个）、`<out>/pack.index.tsv`（逐图指标）、
//! `<out>/pack.manifest.json`（语料内容级指纹 + 配置 + 索引 SHA + 汇总）。
//! 复核器 `ttx-verify` 只信清单与磁盘：重算指纹、逐图比对、重编码比字节。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trit_texture::pack::{Entry, encode, fingerprint_pngs, list_pngs, measure, tsv};
use trit_texture::pngio::load_png_rgba;

const USAGE: &str = "用法：ttx-pack --data <png目录> --out <产包目录> [--limit N] [--mip] [--stride 5] [--run-manifest <json>]";

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

fn git_out(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let data = PathBuf::from(arg(&argv, "--data").ok_or(USAGE)?);
    let out = PathBuf::from(arg(&argv, "--out").ok_or(USAGE)?);
    let limit: usize = arg(&argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mip = argv.iter().any(|a| a == "--mip");
    let stride: usize = arg(&argv, "--stride")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
        .max(1);

    let mut files = list_pngs(&data)?;
    if limit > 0 {
        files.truncate(limit);
    }
    if files.is_empty() {
        return Err(format!("{} 下没有 PNG", data.display()));
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("建不了 {}: {e}", out.display()))?;

    let mut entries: Vec<Entry> = Vec::with_capacity(files.len());
    let (mut checked, mut bad) = (0usize, 0usize);
    for (i, p) in files.iter().enumerate() {
        let img = load_png_rgba(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let ttx = encode(&img.rgba, img.width, img.height, mip)?;
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("文件名非法: {}", p.display()))?;
        let ttx_name = format!("{}.ttx", name.trim_end_matches(".png"));
        std::fs::write(out.join(&ttx_name), &ttx).map_err(|e| format!("写 {ttx_name}: {e}"))?;
        let (psnr, max_err, c, b) = measure(&img.rgba, img.width, img.height, &ttx, stride)?;
        checked += c;
        bad += b;
        entries.push(Entry {
            name: name.to_string(),
            bytes: ttx.len() as u64,
            bpp: ttx.len() as f64 * 8.0 / (img.width * img.height) as f64,
            psnr,
            max_err,
            sha256: trit_core::fingerprint::sha256_hex(&ttx),
        });
        if (i + 1) % 200 == 0 {
            println!("  已编码 {}/{}", i + 1, files.len());
        }
    }

    let index = tsv(&entries);
    std::fs::write(out.join("pack.index.tsv"), &index).map_err(|e| format!("写索引: {e}"))?;
    let index_sha = trit_core::fingerprint::sha256_hex(index.as_bytes());
    let fp = fingerprint_pngs(&files)?;

    let with_psnr: Vec<f64> = entries.iter().filter_map(|e| e.psnr).collect();
    let psnr_mean = if with_psnr.is_empty() {
        f64::NAN
    } else {
        with_psnr.iter().sum::<f64>() / with_psnr.len() as f64
    };
    let bpp_mean = entries.iter().map(|e| e.bpp).sum::<f64>() / entries.len() as f64;
    let max_err = entries.iter().map(|e| e.max_err).max().unwrap_or(0);
    let bytes_total: u64 = entries.iter().map(|e| e.bytes).sum();

    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let commit = git_out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let dirty = !git_out(&["status", "--porcelain"]).trim().is_empty();
    let argv_json: Vec<String> = argv.iter().map(|a| jstr(a)).collect();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"texture-ttx-pack\",\n  \"created_unix\": {created},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"version\": \"ttx2\", \"mip\": {mip}, \"limit\": {limit}, \"sample_stride\": {stride}}},\n  \"data\": {{\"files\": {}, \"bytes\": {}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"root\": {}}},\n  \"index\": {{\"path\": \"pack.index.tsv\", \"files\": {}}},\n  \"index_sha256\": \"{index_sha}\",\n  \"result\": {{\"psnr_mean\": {}, \"bpp_mean\": {}, \"max_err\": {max_err}, \"bytes_total\": {bytes_total}, \"sampling_checked\": {checked}, \"sampling_bad\": {bad}}},\n  \"artifact\": {{\"dir\": {}}}\n}}\n",
        jstr(&commit),
        argv_json.join(", "),
        fp.files,
        fp.bytes,
        fp.hash,
        jstr(&norm(&data)),
        entries.len(),
        jnum(psnr_mean),
        jnum(bpp_mean),
        jstr(&norm(&out))
    );
    let manifest = arg(&argv, "--run-manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| out.join("pack.manifest.json"));
    std::fs::write(&manifest, body).map_err(|e| format!("写清单: {e}"))?;

    println!(
        "包 {}：{} 张，{:.2} dB，{:.3} bpp，{} 字节；索引 {}；清单 {}",
        out.display(),
        entries.len(),
        psnr_mean,
        bpp_mean,
        bytes_total,
        index_sha,
        manifest.display()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ttx-pack：{e}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
