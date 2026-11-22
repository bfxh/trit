//! 数据管线：repo 扫描 → 仓库级质量门 → 分桶 JSONL + 数据卡 + 内容级清单。

use std::path::PathBuf;

use trit_data::gate::Decision;
use trit_data::manifest::dataset_manifest;
use trit_data::repo;
use trit_data::repo_gate::{self, Config};
use trit_learn::json::write_json;

use super::{die, pipeline_report, usage, write_lines};

pub fn run(argv: &[String]) {
    let mut root: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut cfg = Config::default();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--repo" => {
                root = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--require-license" => {
                cfg.require_license = true;
                i += 1;
            }
            "--strict-license" => {
                cfg.reject_missing_license = true;
                i += 1;
            }
            "--require-tests" => {
                cfg.require_tests = true;
                i += 1;
            }
            "--allow-generated" => {
                cfg.reject_generated = false;
                i += 1;
            }
            "--allow-duplicates" => {
                cfg.reject_duplicates = false;
                i += 1;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    let root = root.unwrap_or_else(|| usage());
    let dir = out.unwrap_or_else(|| PathBuf::from("pipeline-out"));
    let units = repo::scan(&root).unwrap_or_else(|e| die(&e));
    let dup = repo::dup::scan(&root, &units);
    let dup_paths = repo_gate::duplicate_paths(&dup);
    let d = repo::deps::summarize(&units);
    let (mut accepted, mut deferred, mut rejected) = (Vec::new(), Vec::new(), Vec::new());
    let mut rows: Vec<(String, &'static str, Vec<&'static str>)> = Vec::new();
    for u in &units {
        let v = repo_gate::evaluate(u, &cfg, &dup_paths);
        let line = pipeline_report::unit_line(u, &v);
        match v.decision {
            Decision::Accept => accepted.push(line),
            Decision::Defer => deferred.push(line),
            Decision::Reject => rejected.push(line),
        }
        rows.push((u.path.clone(), v.decision.as_str(), v.reasons));
    }
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
    write_lines(&dir.join("accepted.jsonl"), &accepted);
    write_lines(&dir.join("deferred.jsonl"), &deferred);
    write_lines(&dir.join("rejected.jsonl"), &rejected);
    let view: Vec<pipeline_report::VerdictRow<'_>> = rows
        .iter()
        .map(|(_, decision, reasons)| pipeline_report::VerdictRow { decision, reasons })
        .collect();
    let root_s = root.display().to_string();
    let report = pipeline_report::report(&root_s, &view, units.len(), &d, &dup);
    write_json(&dir.join("pipeline.report.json"), &report).unwrap_or_else(|e| die(&e));
    let card = pipeline_report::card(&root_s, &units, &view, &d, &dup);
    write_json(&dir.join("dataset_card.json"), &card).unwrap_or_else(|e| die(&e));
    let files: Vec<String> = [
        "accepted.jsonl",
        "deferred.jsonl",
        "rejected.jsonl",
        "pipeline.report.json",
        "dataset_card.json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();    let manifest = dataset_manifest(&dir, &files).unwrap_or_else(|e| die(&e));
    write_json(&dir.join("manifest.json"), &pipeline_report::manifest_json(&manifest))
        .unwrap_or_else(|e| die(&e));
    println!(
        "管线 {}：{} 文件 → accept {} / defer {} / reject {}（{}）",
        root.display(),
        units.len(),
        accepted.len(),
        deferred.len(),
        rejected.len(),
        dir.display()
    );
}
