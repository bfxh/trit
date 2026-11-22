//! `trit-data rewrite`：defer 样本过外部重写器 → 保真检查 → 回同一质量门。
//!
//! fail-closed：调用失败（spawn/exit/timeout/empty）、保真失败（锚点丢失/长度越界）
//! 的样本**保留原文**并进 `rewrite.failed.jsonl`；不做“失败也当改写成功”的降级。

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use trit_core::fingerprint::file_sha256;
use trit_data::gate::Decision;
use trit_data::manifest::dataset_manifest;
use trit_data::rewrite::{CommandRewriter, Fidelity, rewrite_trajectory_with, to_json};
use trit_data::schema::Trajectory;
use trit_learn::json::{Json, write_json};

use super::{created_unix, die, usage, write_lines};

#[path = "rewrite_opts.rs"]
mod rewrite_opts;
#[path = "rewrite_metrics.rs"]
mod rewrite_metrics;

use rewrite_metrics::Metrics;

#[derive(Default)]
struct Counts {
    records: usize,
    rewritten: usize,
    accepted: usize,
    deferred: usize,
    rejected: usize,
    fidelity_failed: usize,
    failed: usize,
}

#[derive(Default)]
struct Buckets {
    accepted: Vec<String>,
    deferred: Vec<String>,
    rejected: Vec<String>,
    failed: Vec<String>,
}

const NAMES: [&str; 4] = [
    "rewritten.accepted.jsonl",
    "rewritten.deferred.jsonl",
    "rewritten.rejected.jsonl",
    "rewrite.failed.jsonl",
];

pub fn run(argv: &[String]) {
    let opts = rewrite_opts::parse(argv);
    let path = opts.jsonl.unwrap_or_else(|| usage());
    let text = super::shared::input::read_text(&path).unwrap_or_else(|e| die(&e));
    let input_sha = file_sha256(&path).unwrap_or_default();
    let cfg = opts.cfg;
    let rw = CommandRewriter::parse(&opts.cmd, Duration::from_secs(opts.timeout_secs))
        .unwrap_or_else(|e| die(&e));

    let mut counts = Counts::default();
    let mut metrics = Metrics::default();
    let mut reasons: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut buckets = Buckets::default();
    let mut seen = BTreeSet::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        counts.records += 1;
        let t = match Json::parse(line).and_then(|j| Trajectory::from_json(&j)) {
            Ok(t) => t,
            Err(_) => {
                counts.failed += 1;
                metrics.fail("parse_error");
                buckets.failed.push(failed(line, "parse_error", "").dump());
                continue;
            }
        };
        match rewrite_trajectory_with(&rw, &t, &cfg, &mut seen) {
            Ok((out, fid, v)) if fid.ok => {
                counts.rewritten += 1;
                metrics.observe(&t, &out);
                let rec = to_json(&out)
                    .put("rewrite", fidelity_json(&fid, &t, &rw))
                    .put("gate", super::json::gate_json(&v));
                match v.decision {
                    Decision::Accept => {
                        counts.accepted += 1;
                        buckets.accepted.push(rec.dump());
                    }
                    Decision::Defer => {
                        counts.deferred += 1;
                        buckets.deferred.push(rec.dump());
                    }
                    Decision::Reject => {
                        counts.rejected += 1;
                        buckets.rejected.push(rec.dump());
                    }
                }
                for r in &v.reasons {
                    *reasons.entry(*r).or_insert(0) += 1;
                }
            }
            Ok((_, fid, _)) => {
                counts.fidelity_failed += 1;
                metrics.fail("fidelity");
                let rec = Json::obj().put("raw", line).put(
                    "rewrite",
                    Json::obj()
                        .put("ok", false)
                        .put("reason", "fidelity")
                        .put("missing", strings(&fid.missing))
                        .put("length_ratio_ok", fid.length_ratio_ok),
                );
                buckets.failed.push(rec.dump());
            }
            Err(e) => {
                counts.failed += 1;
                let kind = e.split(':').next().unwrap_or("error");
                metrics.fail(kind);
                buckets.failed.push(failed(line, kind, &e).dump());
            }
        }
    }

    metrics.print(&counts, &reasons, None);
    if let Some(dir) = opts.out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        for (name, lines) in [
            (NAMES[0], &buckets.accepted),
            (NAMES[1], &buckets.deferred),
            (NAMES[2], &buckets.rejected),
            (NAMES[3], &buckets.failed),
        ] {
            write_lines(&dir.join(name), lines);
        }
        let fp = dataset_manifest(&dir, &NAMES.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .unwrap_or_else(|e| die(&e));
        let report = Json::obj()
            .put("schema", 1u32)
            .put("kind", "trit-data-rewrite")
            .put("created_unix", created_unix())
            .put(
                "input",
                Json::obj()
                    .put("path", path.display().to_string())
                    .put("sha256", input_sha)
                    .put("bytes", text.len() as u64)
                    .put("records", counts.records as u64),
            )
            .put(
                "config",
                Json::obj()
                    .put(
                        "cmd",
                        Json::arr(rw.argv().into_iter().map(Json::str).collect()),
                    )
                    .put("timeout_secs", rw.timeout_secs())
                    .put("lang_policy", cfg.lang_policy.as_str())
                    .put("require_verified", cfg.require_verified),
            )
            .put(
                "counts",
                Json::obj()
                    .put("records", counts.records as u64)
                    .put("rewritten", counts.rewritten as u64)
                    .put("accepted", counts.accepted as u64)
                    .put("deferred", counts.deferred as u64)
                    .put("rejected", counts.rejected as u64)
                    .put("fidelity_failed", counts.fidelity_failed as u64)
                    .put("failed", counts.failed as u64),
            )
            .put("metrics", metrics.json(&counts))
            .put("reasons", super::json::reasons_json(&reasons))
            .put(
                "manifest",
                Json::obj()
                    .put("files", fp.files as u64)
                    .put("bytes", fp.bytes)
                    .put("hash", format!("sha256:{}", fp.hash)),
            );
        let rp = dir.join("rewrite.report.json");
        write_json(&rp, &report).unwrap_or_else(|e| die(&e));
        println!("报告 {}", rp.display());
    }
}

fn fidelity_json(f: &Fidelity, t: &Trajectory, rw: &CommandRewriter) -> Json {
    let orig = trit_data::gate::content_key(t);
    Json::obj()
        .put("ok", f.ok)
        .put("missing", strings(&f.missing))
        .put("length_ratio_ok", f.length_ratio_ok)
        .put("orig_sha256", orig)
        .put("cmd", Json::arr(rw.argv().into_iter().map(Json::str).collect()))
}

fn failed(line: &str, kind: &str, detail: &str) -> Json {
    Json::obj().put("raw", line).put(
        "rewrite",
        Json::obj()
            .put("ok", false)
            .put("reason", kind)
            .put("detail", detail),
    )
}

fn strings(v: &[String]) -> Json {
    Json::arr(v.iter().cloned().map(Json::str).collect())
}
