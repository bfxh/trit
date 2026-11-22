//! `decontaminate` 子命令。

use std::path::PathBuf;

use trit_data::decontaminate::overlap;
use trit_data::schema::Trajectory;
use trit_learn::json::{Json, write_json};

use super::{created_unix, die, usage, write_lines};

pub fn run(argv: &[String]) {
    let mut jsonl: Option<PathBuf> = None;
    let mut bench: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut ngram = 13usize;
    let mut threshold = 0.2f64;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--jsonl" => {
                jsonl = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--bench" => {
                bench = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--ngram" => {
                ngram = argv
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--ngram 需要整数"));
                i += 2;
            }
            "--threshold" => {
                threshold = argv
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--threshold 需要小数"));
                i += 2;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    let jsonl = jsonl.unwrap_or_else(|| usage());
    let bench = bench.unwrap_or_else(|| usage());
    let traces = std::fs::read_to_string(&jsonl)
        .unwrap_or_else(|e| die(&format!("读不了 {}: {e}", jsonl.display())));
    let bench_text = std::fs::read_to_string(&bench)
        .unwrap_or_else(|e| die(&format!("读不了 {}: {e}", bench.display())));
    let refs: Vec<String> = bench_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            Json::parse(l)
                .ok()
                .and_then(|j| j.get_str("text").map(|s| s.to_string()))
                .unwrap_or_else(|| l.to_string())
        })
        .collect();

    let mut flagged = 0usize;
    let mut lines = Vec::new();
    for line in traces.lines().filter(|l| !l.trim().is_empty()) {
        let j = Json::parse(line).unwrap_or_else(|e| die(&format!("{}: {e}", jsonl.display())));
        let t =
            Trajectory::from_json(&j).unwrap_or_else(|e| die(&format!("{}: {e}", jsonl.display())));
        let text = text_of(&t);
        let mut best = (0.0f64, 0usize, false);
        for (idx, r) in refs.iter().enumerate() {
            let o = overlap(&text, r, ngram, threshold);
            if o.ratio > best.0 {
                best = (o.ratio, idx, o.contaminated);
            }
        }
        if best.2 {
            flagged += 1;
        }
        let rec = j.put(
            "contamination",
            Json::obj()
                .put("ngram", ngram as u64)
                .put("ratio", best.0)
                .put("bench_index", best.1 as u64)
                .put("contaminated", best.2),
        );
        lines.push(rec.dump());
    }
    println!(
        "记录 {}：contaminated {}（n={ngram}, threshold={threshold}）",
        lines.len(),
        flagged
    );
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        write_lines(&dir.join("decontaminated.jsonl"), &lines);
        let report = Json::obj()
            .put("schema", 1u32)
            .put("kind", "trit-data-decontaminate")
            .put("created_unix", created_unix())
            .put(
                "input",
                Json::obj().put("path", jsonl.display().to_string()),
            )
            .put(
                "bench",
                Json::obj()
                    .put("path", bench.display().to_string())
                    .put("texts", refs.len() as u64),
            )
            .put("ngram", ngram as u64)
            .put("threshold", threshold)
            .put("records", lines.len() as u64)
            .put("contaminated", flagged as u64);
        write_json(&dir.join("decontaminate.report.json"), &report).unwrap_or_else(|e| die(&e));
    }
}

fn text_of(t: &Trajectory) -> String {
    let mut out = String::new();
    for s in &t.steps {
        if let Some(x) = &s.thought {
            out.push_str(x);
            out.push('\n');
        }
        if let Some(x) = &s.observation {
            out.push_str(x);
            out.push('\n');
        }
    }
    out.push_str(&t.outcome.final_text);
    out
}
