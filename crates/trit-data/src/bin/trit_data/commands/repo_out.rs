//! repo 输出落盘：units / deps / report / dup / closure。

use std::path::Path;

use trit_data::repo;
use trit_data::repo::Unit;
use trit_learn::json::write_json;

use super::{die, repo_dup, repo_json, repo_stats, write_lines};

#[allow(clippy::too_many_arguments)]
pub fn write(
    dir: &Path,
    root: &str,
    units: &[Unit],
    s: &repo_stats::Stats,
    d: &repo::deps::Summary,
    dup: &repo::dup::Dup,
    closure: Option<&str>,
    max_files: usize,
) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
    write_lines(&dir.join("repo.units.jsonl"), &repo_json::units_lines(units));
    let deps = repo_json::deps_document(root, units, d);
    write_json(&dir.join("repo.deps.json"), &deps).unwrap_or_else(|e| die(&e));
    let report = repo_stats::report(root, units, s, d).put("duplicates", repo_dup::json(dup));
    write_json(&dir.join("repo.report.json"), &report).unwrap_or_else(|e| die(&e));
    write_json(&dir.join("repo.dup.json"), &repo_dup::json(dup)).unwrap_or_else(|e| die(&e));
    if let Some(start) = closure {
        write_json(&dir.join("repo.closure.json"), &repo_json::closure_json(units, start, max_files)).unwrap_or_else(|e| die(&e));
    }
}
