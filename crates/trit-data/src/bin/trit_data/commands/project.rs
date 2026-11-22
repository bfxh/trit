//! `project` 子命令：TraceEvent JSONL → 轨迹 JSONL。

use std::path::PathBuf;

use trit_data::project::project_jsonl;
use trit_learn::json::{Json, write_json};

use super::{created_unix, die, usage, write_lines};

pub fn run(argv: &[String]) {
    let mut events: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--events" => {
                events = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    let events = events.unwrap_or_else(|| usage());
    let text = std::fs::read_to_string(&events)
        .unwrap_or_else(|e| die(&format!("读不了 {}: {e}", events.display())));
    let projected = project_jsonl(&text).unwrap_or_else(|e| die(&e));
    let lines: Vec<String> = projected.iter().map(|j| j.dump()).collect();
    println!(
        "轨迹 {} 条（事件 {} 行）",
        lines.len(),
        text.lines().filter(|l| !l.trim().is_empty()).count()
    );
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        write_lines(&dir.join("trajectories.jsonl"), &lines);
        let report = Json::obj()
            .put("schema", 1u32)
            .put("kind", "trit-data-project")
            .put("created_unix", created_unix())
            .put(
                "input",
                Json::obj().put("path", events.display().to_string()),
            )
            .put("trajectories", lines.len() as u64);
        write_json(&dir.join("project.report.json"), &report).unwrap_or_else(|e| die(&e));
    }
}
