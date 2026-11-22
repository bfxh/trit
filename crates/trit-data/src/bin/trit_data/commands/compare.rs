//! `compare` 子命令。

use std::path::{Path, PathBuf};

use trit_data::compare::cohen_kappa;
use trit_learn::json::Json;

use super::{die, usage};

pub fn run(argv: &[String]) {
    let mut a: Option<PathBuf> = None;
    let mut b: Option<PathBuf> = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--a" => {
                a = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--b" => {
                b = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    let a = a.unwrap_or_else(|| usage());
    let b = b.unwrap_or_else(|| usage());
    let la = read_decisions(&a);
    let lb = read_decisions(&b);
    if la.len() != lb.len() {
        die(&format!("样本数不一致：{} vs {}", la.len(), lb.len()));
    }
    let k = cohen_kappa(&la, &lb).unwrap_or_else(|e| die(&e));
    println!(
        "n {}：observed {:.4}，expected {:.4}，Cohen κ {:.4}",
        k.n, k.observed, k.expected, k.kappa
    );
    for ((x, y), n) in &k.counts {
        println!("  {x} / {y}: {n}");
    }
}

fn read_decisions(p: &Path) -> Vec<String> {
    let text =
        std::fs::read_to_string(p).unwrap_or_else(|e| die(&format!("读不了 {}: {e}", p.display())));
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let j = Json::parse(line).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
        let d = decision_of(&j).unwrap_or_else(|| die(&format!("{} 缺 decision", p.display())));
        out.push(d);
    }
    out
}

fn decision_of(j: &Json) -> Option<String> {
    j.get_str("decision")
        .or_else(|| j.get("gate").and_then(|g| g.get_str("decision")))
        .map(|s| s.to_string())
}
