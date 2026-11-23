//! 轨迹级管线：`gate` 三支 → 可选 `rewrite` → `mix` 配额与混样。
//!
//! 仓库级管线（`pipeline --repo`）处理代码文件；这里处理轨迹 JSONL，把原本
//! `gate` / `rewrite` / `mix` 三条散命令收成一次可复算运行。所有阶段产物都在
//! 同一目录，最后写 `trajectory.pipeline.report.json` 与内容级 `manifest.json`。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use trit_core::fingerprint::sha256_hex;
use trit_data::gate::{self, Config, Decision};
use trit_data::manifest::dataset_manifest;
use trit_data::mix::allocate;
use trit_data::rewrite::{CommandRewriter, Fidelity, rewrite_trajectory_with, to_json};
use trit_data::schema::Trajectory;
use trit_learn::json::{Json, write_json};

use super::{created_unix, json::gate_json, write_lines};

const GATE_NAMES: [&str; 3] = ["accepted.jsonl", "deferred.jsonl", "rejected.jsonl"];
const RW_NAMES: [&str; 4] = [
    "rewritten.accepted.jsonl",
    "rewritten.deferred.jsonl",
    "rewritten.rejected.jsonl",
    "rewrite.failed.jsonl",
];

pub struct Opts {
    pub gate: Config,
    pub rewrite_cmd: Option<String>,
    pub rewrite_timeout: u64,
    pub mix_total: Option<usize>,
    pub mix_weights: BTreeMap<String, f64>,
}

pub struct Summary {
    pub input_files: usize,
    pub records: usize,
    pub accept: usize,
    pub defer: usize,
    pub reject: usize,
    pub rewrite_enabled: bool,
    pub rewritten: usize,
    pub mixed: usize,
}

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
struct Metrics {
    failures: BTreeMap<String, usize>,
    ratios: Vec<f64>,
}

impl Metrics {
    fn fail(&mut self, reason: &str) {
        *self.failures.entry(reason.to_string()).or_insert(0) += 1;
    }

    fn observe(&mut self, before: &Trajectory, after: &Trajectory) {
        let a = trit_data::script::trajectory_text(before).chars().count();
        let b = trit_data::script::trajectory_text(after).chars().count();
        if a > 0 {
            self.ratios.push(b as f64 / a as f64);
        }
    }
}

pub fn execute(input: &Path, dir: &Path, opts: &Opts) -> Result<Summary, String> {
    let files = collect_jsonl(input)?;
    if files.is_empty() {
        return Err(format!("{} 下没有 .jsonl", input.display()));
    }
    let mut raw: Vec<(PathBuf, String)> = Vec::with_capacity(files.len());
    let mut combined = Vec::new();
    let mut inputs = Vec::new();
    for path in &files {
        let text = super::shared::input::read_text(path)?;
        let bytes = text.len() as u64;
        combined.extend_from_slice(text.as_bytes());
        inputs.push(
            Json::obj()
                .put("path", path.display().to_string())
                .put("bytes", bytes)
                .put("sha256", sha256_hex(text.as_bytes())),
        );
        raw.push((path.clone(), text));
    }

    std::fs::create_dir_all(dir).map_err(|e| format!("建不了 {}: {e}", dir.display()))?;

    let mut seen = BTreeSet::new();
    let mut decisions = [0usize; 3];
    let mut parse_errors = 0usize;
    let mut reasons: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut buckets: [Vec<String>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut records = 0usize;
    for (_, text) in &raw {
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            records += 1;
            let parsed: Result<(Json, Trajectory), String> =
                Json::parse(line).and_then(|j| Trajectory::from_json(&j).map(|t| (j, t)));
            let (mut rec, t) = match parsed {
                Ok(value) => value,
                Err(_) => {
                    parse_errors += 1;
                    *reasons.entry("parse_error").or_insert(0) += 1;
                    decisions[2] += 1;
                    let rec = Json::obj().put("raw", line).put(
                        "gate",
                        Json::obj()
                            .put("decision", "reject")
                            .put("reasons", Json::arr(vec![Json::str("parse_error")])),
                    );
                    buckets[2].push(rec.dump());
                    continue;
                }
            };
            let verdict = gate::evaluate(&t, &opts.gate, &mut seen);
            let idx = match verdict.decision {
                Decision::Accept => 0,
                Decision::Defer => 1,
                Decision::Reject => 2,
            };
            decisions[idx] += 1;
            for reason in &verdict.reasons {
                *reasons.entry(*reason).or_insert(0) += 1;
            }
            rec = rec.put("gate", gate_json(&verdict));
            buckets[idx].push(rec.dump());
        }
    }
    for (name, lines) in GATE_NAMES.iter().zip(buckets.iter()) {
        write_lines(&dir.join(name), lines);
    }

    let mut rw_counts = Counts::default();
    let mut rw_metrics = Metrics::default();
    let mut rw_gate_reasons: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut rw_buckets = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut rewrite_enabled = false;
    let mut rewriter: Option<CommandRewriter> = None;
    if let Some(cmd) = &opts.rewrite_cmd {
        rewrite_enabled = true;
        let parsed = CommandRewriter::parse(cmd, Duration::from_secs(opts.rewrite_timeout))?;
        rewriter = Some(parsed);
        let rw = rewriter.as_ref().expect("刚写入");
        let mut rw_seen = BTreeSet::new();
        for line in &buckets[1] {
            rw_counts.records += 1;
            let t = match Json::parse(line).and_then(|j| Trajectory::from_json(&j)) {
                Ok(t) => t,
                Err(_) => {
                    rw_counts.failed += 1;
                    rw_metrics.fail("parse_error");
                    rw_buckets[3].push(failed(line, "parse_error", "").dump());
                    continue;
                }
            };
            match rewrite_trajectory_with(rw, &t, &opts.gate, &mut rw_seen) {
                Ok((out, fid, verdict)) if fid.ok => {
                    rw_counts.rewritten += 1;
                    rw_metrics.observe(&t, &out);
                    let rec = to_json(&out)
                        .put("rewrite", fidelity_json(&fid, &t, rw))
                        .put("gate", gate_json(&verdict));
                    let idx = match verdict.decision {
                        Decision::Accept => 0,
                        Decision::Defer => 1,
                        Decision::Reject => 2,
                    };
                    match idx {
                        0 => rw_counts.accepted += 1,
                        1 => rw_counts.deferred += 1,
                        _ => rw_counts.rejected += 1,
                    }
                    for reason in &verdict.reasons {
                        *rw_gate_reasons.entry(*reason).or_insert(0) += 1;
                    }
                    rw_buckets[idx].push(rec.dump());
                }
                Ok((_, fid, _)) => {
                    rw_counts.fidelity_failed += 1;
                    rw_metrics.fail("fidelity");
                    let rec = Json::obj().put("raw", line).put(
                        "rewrite",
                        Json::obj()
                            .put("ok", false)
                            .put("reason", "fidelity")
                            .put("missing", strings(&fid.missing))
                            .put("length_ratio_ok", fid.length_ratio_ok),
                    );
                    rw_buckets[3].push(rec.dump());
                }
                Err(error) => {
                    rw_counts.failed += 1;
                    let kind = error.split(':').next().unwrap_or("error");
                    rw_metrics.fail(kind);
                    rw_buckets[3].push(failed(line, kind, &error).dump());
                }
            }
        }
        for (name, lines) in RW_NAMES.iter().zip(rw_buckets.iter()) {
            write_lines(&dir.join(name), lines);
        }
        println!(
            "重写：{} 条 deferred → 成功 {}（accept {} / defer {} / reject {}）/ 保真失败 {} / 调用失败 {}",
            rw_counts.records,
            rw_counts.rewritten,
            rw_counts.accepted,
            rw_counts.deferred,
            rw_counts.rejected,
            rw_counts.fidelity_failed,
            rw_counts.failed
        );
    }
    let final_accepted: Vec<String> = buckets[0]
        .iter()
        .chain(rw_buckets[0].iter())
        .cloned()
        .collect();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in &final_accepted {
        let j = Json::parse(line)?;
        let t = Trajectory::from_json(&j)?;
        let label = t
            .labels
            .first()
            .cloned()
            .unwrap_or_else(|| "unlabeled".to_string());
        *counts.entry(label).or_insert(0) += 1;
    }
    let total = opts.mix_total.unwrap_or(final_accepted.len());
    let quotas = allocate(&counts, total, &opts.mix_weights);
    let mut left: BTreeMap<String, usize> =
        quotas.iter().map(|q| (q.label.clone(), q.take)).collect();
    let mut mixed = Vec::new();
    for line in &final_accepted {
        let j = Json::parse(line)?;
        let t = Trajectory::from_json(&j)?;
        let label = t
            .labels
            .first()
            .cloned()
            .unwrap_or_else(|| "unlabeled".to_string());
        if let Some(slot) = left.get_mut(&label)
            && *slot > 0
        {
            *slot -= 1;
            mixed.push(line.clone());
        }
    }
    let selected = mixed.len();
    let shortfall = total.saturating_sub(selected);
    write_lines(&dir.join("mixed.jsonl"), &mixed);
    let mix_report = Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-mix")
        .put("total", total as u64)
        .put("available", final_accepted.len() as u64)
        .put("selected", selected as u64)
        .put("shortfall", shortfall as u64)
        .put(
            "quotas",
            Json::arr(
                quotas
                    .iter()
                    .map(|q| {
                        Json::obj()
                            .put("label", q.label.clone())
                            .put("available", q.available as u64)
                            .put("weight", num_f(q.weight))
                            .put("take", q.take as u64)
                    })
                    .collect(),
            ),
        );
    write_json(&dir.join("mix.json"), &mix_report)?;

    let mut manifest_files: Vec<String> = GATE_NAMES.iter().map(|s| s.to_string()).collect();
    if rewrite_enabled {
        manifest_files.extend(RW_NAMES.iter().map(|s| s.to_string()));
    }
    manifest_files.push("mixed.jsonl".to_string());
    manifest_files.push("mix.json".to_string());
    let fp = dataset_manifest(dir, &manifest_files)?;
    write_json(
        &dir.join("manifest.json"),
        &super::pipeline_report::manifest_json(&fp),
    )?;

    let mut report = Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-trajectory-pipeline")
        .put("created_unix", created_unix())
        .put(
            "input",
            Json::obj()
                .put("path", input.display().to_string())
                .put("files", Json::arr(inputs))
                .put("bytes", combined.len() as u64)
                .put("sha256", sha256_hex(&combined))
                .put("records", records as u64)
                .put("parse_errors", parse_errors as u64),
        )
        .put(
            "config",
            Json::obj()
                .put(
                    "gate",
                    Json::obj()
                        .put("require_verified", opts.gate.require_verified)
                        .put("max_steps", opts.gate.max_steps as u64)
                        .put("max_tokens", opts.gate.max_tokens)
                        .put("max_tool_calls", opts.gate.max_tool_calls)
                        .put("lang_policy", opts.gate.lang_policy.as_str()),
                )
                .put(
                    "rewrite",
                    Json::obj()
                        .put("enabled", rewrite_enabled)
                        .put("cmd", opts.rewrite_cmd.clone().unwrap_or_default())
                        .put("timeout_secs", opts.rewrite_timeout),
                )
                .put(
                    "mix",
                    Json::obj()
                        .put("total", total as u64)
                        .put("weights", weights_json(&opts.mix_weights)),
                ),
        )
        .put(
            "counts",
            Json::obj()
                .put("records", records as u64)
                .put("accepted", decisions[0] as u64)
                .put("deferred", decisions[1] as u64)
                .put("rejected", decisions[2] as u64)
                .put("parse_errors", parse_errors as u64),
        )
        .put(
            "gate",
            Json::obj()
                .put("accept", decisions[0] as u64)
                .put("defer", decisions[1] as u64)
                .put("reject", decisions[2] as u64)
                .put("reasons", super::json::reasons_json(&reasons))
                .put(
                    "rates",
                    Json::obj()
                        .put("accept", num_s(frac(decisions[0], records)))
                        .put("defer", num_s(frac(decisions[1], records)))
                        .put("reject", num_s(frac(decisions[2], records))),
                ),
        )
        .put(
            "rewrite",
            match &rewriter {
                Some(rewriter) => Json::obj()
                    .put("enabled", true)
                    .put("counts", rewrite_counts_json(&rw_counts))
                    .put(
                        "metrics",
                        rewrite_metrics_json(&rw_counts, &rw_metrics, &rw_gate_reasons),
                    )
                    .put("cmd", Json::arr(rewriter.argv().into_iter().map(Json::str).collect())),
                None => Json::obj().put("enabled", false),
            },
        )
        .put("mix", mix_report)
        .put(
            "manifest",
            Json::obj()
                .put("files", fp.files as u64)
                .put("bytes", fp.bytes)
                .put("hash", format!("sha256:{}", fp.hash)),
        );
    // `mix` 已写进报告，避免重复大对象；这里只保留一次。
    report = report.put("artifacts", Json::arr(
        manifest_files.iter().cloned().map(Json::str).collect(),
    ));
    write_json(&dir.join("trajectory.pipeline.report.json"), &report)?;

    Ok(Summary {
        input_files: files.len(),
        records,
        accept: decisions[0],
        defer: decisions[1],
        reject: decisions[2],
        rewrite_enabled,
        rewritten: rw_counts.rewritten,
        mixed: selected,
    })
}

fn collect_jsonl(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        return Err(format!("{} 不是文件或目录", path.display()));
    }
    let mut out = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("读 {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("读 {}: {e}", dir.display()))?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
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

fn fidelity_json(f: &Fidelity, t: &Trajectory, rw: &CommandRewriter) -> Json {
    Json::obj()
        .put("ok", f.ok)
        .put("missing", strings(&f.missing))
        .put("length_ratio_ok", f.length_ratio_ok)
        .put("orig_sha256", trit_data::gate::content_key(t))
        .put(
            "cmd",
            Json::arr(rw.argv().into_iter().map(Json::str).collect()),
        )
}

fn weights_json(weights: &BTreeMap<String, f64>) -> Json {
    let mut out = Json::obj();
    for (label, weight) in weights {
        out = out.put(label.clone(), num_f(*weight));
    }
    out
}

fn rewrite_counts_json(counts: &Counts) -> Json {
    Json::obj()
        .put("records", counts.records as u64)
        .put("rewritten", counts.rewritten as u64)
        .put("accepted", counts.accepted as u64)
        .put("deferred", counts.deferred as u64)
        .put("rejected", counts.rejected as u64)
        .put("fidelity_failed", counts.fidelity_failed as u64)
        .put("failed", counts.failed as u64)
}

fn rewrite_metrics_json(
    counts: &Counts,
    metrics: &Metrics,
    gate_reasons: &BTreeMap<&'static str, usize>,
) -> Json {
    let attempted = counts.rewritten + counts.fidelity_failed;
    let total = counts.accepted + counts.deferred + counts.rejected;
    let mut sorted = metrics.ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut failures = Vec::new();
    for (reason, count) in &metrics.failures {
        failures.push(
            Json::obj()
                .put("reason", reason.clone())
                .put("count", *count as u64),
        );
    }
    Json::obj()
        .put(
            "fidelity",
            Json::obj()
                .put("attempted", attempted as u64)
                .put("passed", counts.rewritten as u64)
                .put("rate", num_s(frac(counts.rewritten, attempted))),
        )
        .put(
            "gate_decisions",
            Json::obj()
                .put("accept", counts.accepted as u64)
                .put("defer", counts.deferred as u64)
                .put("reject", counts.rejected as u64)
                .put("accept_share", num_s(frac(counts.accepted, total)))
                .put("defer_share", num_s(frac(counts.deferred, total))),
        )
        .put(
            "length_ratio",
            Json::obj()
                .put(
                    "p10",
                    percentile(&mut sorted, 0.1)
                        .map(num_f)
                        .unwrap_or(Json::Null),
                )
                .put(
                    "p50",
                    percentile(&mut sorted, 0.5)
                        .map(num_f)
                        .unwrap_or(Json::Null),
                )
                .put(
                    "p90",
                    percentile(&mut sorted, 0.9)
                        .map(num_f)
                        .unwrap_or(Json::Null),
                )
                .put("samples", sorted.len() as u64),
        )
        .put("failure_reasons", Json::Arr(failures))
        .put("gate_reasons", super::json::reasons_json(gate_reasons))
}

fn percentile(sorted: &mut [f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let pos = q * (sorted.len() - 1) as f64;
    let lower = pos.floor() as usize;
    let upper = pos.ceil() as usize;
    if lower == upper {
        return Some(sorted[lower]);
    }
    let weight = pos - lower as f64;
    Some(sorted[lower] * (1.0 - weight) + sorted[upper] * weight)
}

fn num_s(raw: String) -> Json {
    Json::Num(raw)
}

fn num_f(value: f64) -> Json {
    Json::Num(format!("{value:.6}"))
}

fn frac(part: usize, total: usize) -> String {
    if total == 0 {
        "0.000000".to_string()
    } else {
        format!("{:.6}", part as f64 / total as f64)
    }
}
