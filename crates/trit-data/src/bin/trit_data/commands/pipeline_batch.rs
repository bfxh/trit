//! 批量管线：多仓库配置 + 与基线的变化对比 + 基线更新。

use std::path::Path;

use trit_data::repo_gate::Config;
use trit_learn::json::{read_json, write_json, Json};

use super::{pipeline_diff, pipeline_run};

pub fn run(config: &Path, out: &Path, promote: bool) -> Result<(), String> {
    let cfg = read_json(config)?;
    let repos = cfg
        .get("repos")
        .and_then(Json::as_array)
        .ok_or_else(|| "配置缺少 repos 数组".to_string())?;
    let mut entries = Vec::new();
    for (i, r) in repos.iter().enumerate() {
        let name = r
            .get_str("name")
            .ok_or_else(|| format!("repos[{i}] 缺少 name"))?;
        let path = r
            .get_str("path")
            .ok_or_else(|| format!("repos[{i}] 缺少 path"))?;
        let base = out.join(name);
        let current = base.join("current");
        let s = pipeline_run::execute(Path::new(path), &current, &gate(r))?;
        let changes = if base.join("baseline/accepted.jsonl").exists() {
            let c = pipeline_diff::compare(&base.join("baseline"), &current)?;
            write_json(&base.join("changes.json"), &c)?;
            c
        } else {
            Json::obj().put("baseline", "missing")
        };
        if promote {
            copy_dir(&current, &base.join("baseline"))?;
        }
        entries.push(
            Json::obj()
                .put("name", name.to_string())
                .put("path", path.to_string())
                .put("files", s.files as u64)
                .put("accept", s.accept as u64)
                .put("defer", s.defer as u64)
                .put("reject", s.reject as u64)
                .put("changes", changes),
        );
    }
    let report = Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-batch")
        .put("out", out.display().to_string())
        .put("repos", Json::arr(entries));
    write_json(&out.join("batch.report.json"), &report)
}

fn gate(r: &Json) -> Config {
    let f = |k: &str| r.get(k).and_then(Json::as_bool).unwrap_or(false);
    Config {
        require_tests: f("require_tests"),
        require_license: f("require_license"),
        reject_missing_license: f("strict_license"),
        ..Config::default()
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("建不了 {}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("读不了 {}: {e}", from.display()))? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_file() {
            let name = p.file_name().ok_or_else(|| "文件名缺失".to_string())?;
            std::fs::copy(&p, to.join(name)).map_err(|e| format!("复制 {}: {e}", p.display()))?;
        }
    }
    Ok(())
}
