//! 并行扩展基准：jobs = 1 / 2 / 4 / 8，输出吞吐、加速比与结果校验和。
//!
//! 运行：`cargo bench -p trit-fmt --bench threads`
//! 语料用 fixtures 重复到 4 MB：文档粒度大一点，线程调度开销才不占主导。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;
use trit_fmt::{available_jobs, format_json, format_many};

const ROUNDS: usize = 5;
const TARGET_BYTES: usize = 4 * 1024 * 1024;
const JOBS: [usize; 4] = [1, 2, 4, 8];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/json")
}

fn sources() -> Vec<String> {
    let dir = fixtures();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("读 fixtures").flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.ends_with(".json") && !name.ends_with(".expected.json") {
            files.push(std::fs::read_to_string(&path).expect("读输入"));
        }
    }
    files.sort();
    files
}

fn corpus() -> Vec<String> {
    let base = sources();
    let mut docs = Vec::new();
    let mut bytes = 0usize;
    while bytes < TARGET_BYTES {
        for doc in &base {
            bytes += doc.len();
            docs.push(doc.clone());
            if bytes >= TARGET_BYTES {
                break;
            }
        }
    }
    docs
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

fn main() {
    let docs = corpus();
    let bytes: usize = docs.iter().map(String::len).sum();
    let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    println!(
        "{}",
        json!({"corpus_bytes": bytes, "docs": docs.len(), "available_jobs": available_jobs()})
    );

    let (serial_elapsed, serial_checksum) = measure(|| {
        let mut total = 0usize;
        for doc in &refs {
            total += format_json(doc).expect("fixtures 必须合法").len();
        }
        total
    });
    println!(
        "{}",
        json!({
            "case": "serial_baseline",
            "jobs": 1,
            "bytes": bytes,
            "us": serial_elapsed.as_micros().max(1),
            "mib_per_s": round2(bytes as f64 / 1048576.0 / (serial_elapsed.as_secs_f64().max(1e-9))),
            "speedup": 1.0,
        })
    );

    for jobs in JOBS {
        let (elapsed, checksum) = measure(|| {
            let results = format_many(&refs, jobs);
            results
                .iter()
                .map(|result| result.as_ref().expect("fixtures 必须合法").len())
                .sum()
        });
        assert_eq!(checksum, serial_checksum, "jobs={jobs} 的输出与串行不一致");
        println!(
            "{}",
            json!({
                "case": "format_many",
                "jobs": jobs,
                "bytes": bytes,
                "us": elapsed.as_micros().max(1),
                "mib_per_s": round2(bytes as f64 / 1048576.0 / (elapsed.as_secs_f64().max(1e-9))),
                "speedup": round2(serial_elapsed.as_secs_f64() / elapsed.as_secs_f64().max(1e-9)),
            })
        );
    }

    println!("threads: ok");
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
