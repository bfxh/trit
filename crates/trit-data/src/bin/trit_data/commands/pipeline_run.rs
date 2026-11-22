//! 单仓库管线执行：扫描 → 仓库级门 → 分桶 JSONL → 报告/数据卡/清单。

use std::path::Path;

use trit_data::gate::Decision;
use trit_data::manifest::dataset_manifest;
use trit_data::repo;
use trit_data::repo_gate::{self, Config};
use trit_learn::json::write_json;

use super::{pipeline_report, write_lines};

pub struct Summary {
    pub files: usize,
    pub accept: usize,
    pub defer: usize,
    pub reject: usize,
}

pub fn execute(root: &Path, dir: &Path, cfg: &Config) -> Result<Summary, String> {
    let units = repo::scan(root)?;
    let dup = repo::dup::scan(root, &units);
    let dup_paths = repo_gate::duplicate_paths(&dup);
    let d = repo::deps::summarize(&units);
    let (mut accepted, mut deferred, mut rejected) = (Vec::new(), Vec::new(), Vec::new());
    let mut rows: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for u in &units {
        let v = repo_gate::evaluate(u, cfg, &dup_paths);
        let line = pipeline_report::unit_line(u, &v);
        match v.decision {
            Decision::Accept => accepted.push(line),
            Decision::Defer => deferred.push(line),
            Decision::Reject => rejected.push(line),
        }
        rows.push((v.decision.as_str(), v.reasons));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("建不了 {}: {e}", dir.display()))?;
    write_lines(&dir.join("accepted.jsonl"), &accepted);
    write_lines(&dir.join("deferred.jsonl"), &deferred);
    write_lines(&dir.join("rejected.jsonl"), &rejected);
    let view: Vec<pipeline_report::VerdictRow<'_>> = rows
        .iter()
        .map(|(decision, reasons)| pipeline_report::VerdictRow { decision, reasons })
        .collect();
    let root_s = root.display().to_string();
    let report = pipeline_report::report(&root_s, &view, units.len(), &d, &dup);
    write_json(&dir.join("pipeline.report.json"), &report)?;
    let card = pipeline_report::card(&root_s, &units, &view, &d, &dup);
    write_json(&dir.join("dataset_card.json"), &card)?;
    let files: Vec<String> = [
        "accepted.jsonl",
        "deferred.jsonl",
        "rejected.jsonl",
        "pipeline.report.json",
        "dataset_card.json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let manifest = dataset_manifest(dir, &files)?;
    write_json(&dir.join("manifest.json"), &pipeline_report::manifest_json(&manifest))?;
    Ok(Summary {
        files: units.len(),
        accept: accepted.len(),
        defer: deferred.len(),
        reject: rejected.len(),
    })
}
