//! `mix` 子命令。

use std::collections::BTreeMap;

use trit_data::mix::allocate;
use trit_data::schema::Trajectory;
use trit_data::script::trajectory_text;
use trit_learn::json::{Json, write_json};

use super::{die, usage};

#[path = "mix_opts.rs"]
mod mix_opts;

pub fn run(argv: &[String]) {
    let opts = mix_opts::parse(argv);
    let jsonl = opts.jsonl.unwrap_or_else(|| usage());
    let mut skipped = 0usize;
    let text = std::fs::read_to_string(&jsonl)
        .unwrap_or_else(|e| die(&format!("读不了 {}: {e}", jsonl.display())));
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let j = Json::parse(line).unwrap_or_else(|e| die(&format!("{}: {e}", jsonl.display())));
        let t =
            Trajectory::from_json(&j).unwrap_or_else(|e| die(&format!("{}: {e}", jsonl.display())));
        if opts.filter.eval(&trajectory_text(&t)).filtered() {
            skipped += 1;
            continue;
        }
        let label = t
            .labels
            .first()
            .cloned()
            .unwrap_or_else(|| "unlabeled".to_string());
        *counts.entry(label).or_insert(0) += 1;
    }
    let quotas = allocate(&counts, opts.total, &opts.weights);
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
        .put("total", opts.total as u64)
        .put("quotas", Json::arr(arr))
        .put("skipped_content", skipped as u64);
    if let Some(dir) = opts.out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        let p = dir.join("mix.json");
        write_json(&p, &report).unwrap_or_else(|e| die(&e));
        println!("配比 {}", p.display());
    } else {
        println!("{}", report.dump());
    }
}
