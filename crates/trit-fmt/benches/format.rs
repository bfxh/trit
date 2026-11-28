//! 吞吐基准：与 prettier 3.10.1 基线（1.29–1.31 MiB/s，见
//! `docs/specs/FRONTIER-FORMATTER-TASK.md` 的"基线"节）同口径对比。
//!
//! 运行：`cargo bench -p trit-fmt --bench format`
//! 每个用例输出一行 JSON（`case` / `bytes` / `us` / `mib_per_s`）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;
use trit_fmt::format_json;

const ROUNDS: usize = 5;
const TARGET_BYTES: usize = 1_300_000;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/json")
}

fn corpus() -> Vec<String> {
    let dir = fixtures();
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("读 fixtures").flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !name.ends_with(".json") || name.ends_with(".expected.json") {
            continue;
        }
        sources.push(std::fs::read_to_string(&path).expect("读输入"));
    }
    // 把语料重复到目标规模，模拟真实批量格式化的吞吐。
    let mut expanded = Vec::new();
    let mut bytes = 0usize;
    while bytes < TARGET_BYTES {
        for source in &sources {
            bytes += source.len();
            expanded.push(source.clone());
            if bytes >= TARGET_BYTES {
                break;
            }
        }
    }
    expanded
}

fn measure(mut run: impl FnMut() -> usize) -> (Duration, usize) {
    std::hint::black_box(run());
    let mut best = Duration::MAX;
    let mut checksum = 0usize;
    for _ in 0..ROUNDS {
        let started = Instant::now();
        checksum = std::hint::black_box(run());
        best = best.min(started.elapsed());
    }
    (best, checksum)
}

fn report(case: &str, bytes: usize, elapsed: Duration, docs: usize) {
    let us = elapsed.as_micros().max(1);
    let mib_per_s = bytes as f64 / (1024.0 * 1024.0) / (us as f64 / 1_000_000.0);
    println!(
        "{}",
        json!({
            "case": case,
            "docs": docs,
            "bytes": bytes,
            "us": us,
            "mib_per_s": (mib_per_s * 100.0).round() / 100.0,
        })
    );
}

fn main() {
    let docs = corpus();
    let bytes: usize = docs.iter().map(String::len).sum();
    println!("{}", json!({"corpus_bytes": bytes, "docs": docs.len()}));

    let (elapsed, checksum) = measure(|| {
        let mut total = 0usize;
        for doc in &docs {
            total += format_json(doc).expect("fixtures 必须合法").len();
        }
        total
    });
    report("format_json", bytes, elapsed, docs.len());

    // 幂等路径（真实使用里更常见：已格式化的文件再跑一遍）。
    let formatted: Vec<String> = docs
        .iter()
        .map(|doc| format_json(doc).expect("fixtures 必须合法"))
        .collect();
    let (elapsed, _) = measure(|| {
        let mut total = 0usize;
        for doc in &formatted {
            total += format_json(doc).expect("已格式化输出必须合法").len();
        }
        total
    });
    report("format_json_idempotent", bytes, elapsed, docs.len());

    // 大文档：用 fixtures 里的 big.json（84 KB），与 prettier 的同文件读数对比。
    let big_path = fixtures().join("big.json");
    let big = std::fs::read_to_string(&big_path).expect("读 big.json");
    let (elapsed, out_len) = measure(|| format_json(&big).expect("big.json 必须合法").len());
    report("format_json_big", big.len(), elapsed, 1);
    assert!(out_len > big.len(), "格式化输出不应比输入短这么多");

    assert!(checksum > 0, "校验和不能为 0（防止优化器删掉工作）");
    println!("format: ok");
}
