//! `gate` 子命令。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::fingerprint::file_sha256;
use trit_data::gate::{Decision, evaluate};
use trit_data::manifest::dataset_manifest;
use trit_data::schema::Trajectory;
use trit_learn::json::{Json, write_json};

use super::card::Card;
use super::json::{gate_json, reasons_json};
use super::{created_unix, die, usage, write_lines};

#[path = "gate_opts.rs"]
mod gate_opts;

pub fn run(argv: &[String]) {
    let opts = gate_opts::parse(argv);
    let out = opts.out;
    let cfg = opts.cfg;
    let path = opts.jsonl.unwrap_or_else(|| usage());
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| die(&format!("读不了 {}: {e}", path.display())));
    let input_bytes = text.len() as u64;
    let input_sha = file_sha256(&path).unwrap_or_default();

    let mut seen = BTreeSet::new();
    let mut card = Card::default();
    let mut decisions = [0usize; 3];
    let mut reasons: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut parse_errors = 0usize;
    let mut buckets: [Vec<String>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut records = 0usize;

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        records += 1;
        let parsed: Result<(Json, Trajectory), String> =
            Json::parse(line).and_then(|j| Trajectory::from_json(&j).map(|t| (j, t)));
        let (mut rec, t) = match parsed {
            Ok(v) => v,
            Err(_) => {
                parse_errors += 1;
                *reasons.entry("parse_error").or_insert(0) += 1;
                decisions[1] += 1;
                let rec = Json::obj().put("raw", line).put(
                    "gate",
                    Json::obj()
                        .put("decision", "reject")
                        .put("reasons", Json::arr(vec![Json::str("parse_error")])),
                );
                buckets[1].push(rec.dump());
                continue;
            }
        };
        let v = evaluate(&t, &cfg, &mut seen);
        card.add(&t);
        let idx = match v.decision {
            Decision::Accept => 0,
            Decision::Reject => 1,
            Decision::Defer => 2,
        };
        decisions[idx] += 1;
        for r in &v.reasons {
            *reasons.entry(*r).or_insert(0) += 1;
        }
        rec = rec.put("gate", gate_json(&v));
        buckets[idx].push(rec.dump());
    }

    println!(
        "记录 {records}：accept {} / reject {} / defer {}；parse_error {parse_errors}",
        decisions[0], decisions[1], decisions[2]
    );
    if !reasons.is_empty() {
        let s: Vec<String> = reasons.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!("原因：{}", s.join(", "));
    }

    if let Some(dir) = out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        let names = ["accepted.jsonl", "rejected.jsonl", "deferred.jsonl"];
        for (name, lines) in names.iter().zip(buckets.iter()) {
            write_lines(&dir.join(name), lines);
        }
        let fp = dataset_manifest(
            &dir,
            &names.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap_or_else(|e| die(&e));
        let report = Json::obj()
            .put("schema", 1u32)
            .put("kind", "trit-data-gate")
            .put("created_unix", created_unix())
            .put(
                "input",
                Json::obj()
                    .put("path", path.display().to_string())
                    .put("sha256", input_sha)
                    .put("bytes", input_bytes)
                    .put("records", records as u64)
                    .put("parse_errors", parse_errors as u64),
            )
            .put(
                "config",
                Json::obj()
                    .put("require_verified", cfg.require_verified)
                    .put("max_steps", cfg.max_steps as u64)
                    .put("max_tokens", cfg.max_tokens)
                    .put("max_tool_calls", cfg.max_tool_calls),
            )
            .put(
                "decisions",
                Json::obj()
                    .put("accept", decisions[0] as u64)
                    .put("reject", decisions[1] as u64)
                    .put("defer", decisions[2] as u64),
            )
            .put("reasons", reasons_json(&reasons))
            .put("dataset_card", card.json())
            .put(
                "manifest",
                Json::obj()
                    .put("files", fp.files as u64)
                    .put("bytes", fp.bytes)
                    .put("hash", format!("sha256:{}", fp.hash)),
            );
        let rp = dir.join("gate.report.json");
        write_json(&rp, &report).unwrap_or_else(|e| die(&e));
        println!("报告 {}", rp.display());
    }
}
