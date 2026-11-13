//! YARA 语义向量回放（P0 可复现最小集：79 条 expr + 4 条 rule_level）。
//!
//! 判法见 `docs/YARA-EXPR/04-CODE-MAP.md` §5：一致 / 弃权 / 错判。
//! 向量由 `tools/gen_vectors.py` 调官方 yara-python 4.5.4 冻结；不要手改。
//! 接缝占位阶段 `parse_rules_v2` 一律 `Err` ⇒ 官方 ERR 一致、其余弃权。

use trit_core::value::Ternary;
use trit_re::sigscan::SigScan;
use trit_re::yaraparse::{RuleOutcome, parse_rules_v2};

/// 弃权上限：只许调低。当前 = 83 条里官方非 ERR 的条数。
const GAP_CEILING: usize = 74;

enum Out {
    Consistent,
    Abstain,
    Wrong,
}

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("yara_expr")
}

fn rows(name: &str) -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(root().join(name)).expect("读向量");
    text.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

fn fixture(name: &str) -> Vec<u8> {
    let text = std::fs::read_to_string(root().join("fixtures").join(format!("{name}.hex")))
        .expect("读 fixture");
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let b = line.trim().as_bytes();
        let mut i = 0;
        while i + 1 < b.len() {
            out.push(u8::from_str_radix(&line.trim()[i..i + 2], 16).expect("hex"));
            i += 2;
        }
    }
    out
}

fn expr_source(strings: &str, cond: &str) -> String {
    let mut s = String::from("rule v {\n");
    if strings != "-" {
        s.push_str("  strings:\n");
        for part in strings.split("  ").filter(|p| !p.is_empty()) {
            s.push_str("    ");
            s.push_str(part);
            s.push('\n');
        }
    }
    s.push_str("  condition:\n    ");
    s.push_str(cond);
    s.push_str("\n}\n");
    s
}

fn scanned_hits(source: &str, data: &[u8]) -> Option<Vec<String>> {
    let file = parse_rules_v2(source).ok()?;
    if file.rules.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for outcome in &file.rules {
        match outcome {
            RuleOutcome::Unsupported { .. } => return None,
            RuleOutcome::Parsed(rule) => {
                let report = SigScan::new(std::slice::from_ref(rule)).scan(data, false);
                if report.value == Ternary::True {
                    names.push(rule.name.clone());
                }
            }
        }
    }
    Some(names)
}

fn replay_expr(row: &[String]) -> Out {
    let (id, fixture_name, strings, cond, official) = (&row[0], &row[1], &row[2], &row[3], &row[4]);
    let _ = id;
    let data = fixture(fixture_name);
    let hit = match scanned_hits(&expr_source(strings, cond), &data) {
        None => {
            return if official == "ERR" {
                Out::Consistent
            } else {
                Out::Abstain
            };
        }
        Some(names) => !names.is_empty(),
    };
    match official.as_str() {
        "ERR" => Out::Wrong,
        "HIT" if hit => Out::Consistent,
        "MISS" if !hit => Out::Consistent,
        _ => Out::Wrong,
    }
}

fn replay_rule(row: &[String]) -> Out {
    let (fixture_name, source, official) = (&row[1], &row[2], &row[3]);
    let data = fixture(fixture_name);
    let got = match scanned_hits(source, &data) {
        None => {
            return if official == "ERR" {
                Out::Consistent
            } else {
                Out::Abstain
            };
        }
        Some(names) => names,
    };
    if official == "ERR" {
        return Out::Wrong;
    }
    let want: Vec<&str> = official
        .strip_prefix("HITS:")
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .collect();
    let mut a = got;
    a.sort();
    let mut w: Vec<&str> = want;
    w.sort();
    if a == w { Out::Consistent } else { Out::Wrong }
}

#[test]
fn yara_expr_vectors_replay() {
    let show = std::env::var("YARA_EXPR_SHOW").unwrap_or_default();
    let mut consistent = 0usize;
    let mut abstain = 0usize;
    let mut wrong = Vec::new();

    for row in rows("expr_core.tsv") {
        let id = row[0].clone();
        match replay_expr(&row) {
            Out::Consistent => consistent += 1,
            Out::Abstain => {
                abstain += 1;
                if show.is_empty() || id.starts_with(&show) {
                    eprintln!("弃权 {id}: {}", row[3]);
                }
            }
            Out::Wrong => wrong.push(id),
        }
    }
    for row in rows("rule_level.tsv") {
        let id = row[0].clone();
        match replay_rule(&row) {
            Out::Consistent => consistent += 1,
            Out::Abstain => abstain += 1,
            Out::Wrong => wrong.push(id),
        }
    }

    eprintln!("yara_expr_vectors: 一致 {consistent} / 弃权 {abstain}（上限 {GAP_CEILING}）");
    assert!(wrong.is_empty(), "错判：{wrong:?}");
    assert!(
        abstain <= GAP_CEILING,
        "弃权 {abstain} 超过上限 {GAP_CEILING}"
    );
}
