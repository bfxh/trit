//! `mix` 子命令。

use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_data::mix::allocate;
use trit_data::schema::Trajectory;
use trit_learn::json::{Json, write_json};

use super::{die, usage};

pub fn run(argv: &[String]) {
    let mut jsonl: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut total = 1000usize;
    let mut weights: BTreeMap<String, f64> = BTreeMap::new();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--jsonl" => {
                jsonl = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--total" => {
                total = argv
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--total 需要整数"));
                i += 2;
            }
            "--weights" => {
                for part in argv.get(i + 1).map(|s| s.as_str()).unwrap_or("").split(',') {
                    let (k, v) = part
                        .split_once('=')
                        .unwrap_or_else(|| die("--weights 形如 code=0.7,writing=0.3"));
                    weights.insert(
                        k.to_string(),
                        v.parse().unwrap_or_else(|_| die("权重需要小数")),
                    );
                }
                i += 2;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    let jsonl = jsonl.unwrap_or_else(|| usage());
    let text = std::fs::read_to_string(&jsonl)
        .unwrap_or_else(|e| die(&format!("读不了 {}: {e}", jsonl.display())));
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let j = Json::parse(line).unwrap_or_else(|e| die(&format!("{}: {e}", jsonl.display())));
        let t =
            Trajectory::from_json(&j).unwrap_or_else(|e| die(&format!("{}: {e}", jsonl.display())));
        let label = t
            .labels
            .first()
            .cloned()
            .unwrap_or_else(|| "unlabeled".to_string());
        *counts.entry(label).or_insert(0) += 1;
    }
    let quotas = allocate(&counts, total, &weights);
    let arr: Vec<Json> = quotas
        .iter()
        .map(|q| {
            Json::obj()
                .put("label", q.label.clone())
                .put("available", q.available as u64)
                .put("weight", q.weight)
                .put("take", q.take as u64)
        })
        .collect();
    let report = Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-mix")
        .put("total", total as u64)
        .put("quotas", Json::arr(arr));
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        let p = dir.join("mix.json");
        write_json(&p, &report).unwrap_or_else(|e| die(&e));
        println!("配比 {}", p.display());
    } else {
        println!("{}", report.dump());
    }
}
