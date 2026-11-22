//! 与基线的变化：清单哈希、决策/原因 diff、accepted 文件增删。

use std::collections::BTreeSet;
use std::path::Path;

use trit_learn::json::{read_json, Json};

const DECISIONS: &[&str] = &["accept", "defer", "reject"];
const REASONS: &[&str] = &[
    "schema",
    "unknown_lang",
    "generated",
    "duplicate",
    "license",
    "imports_unknown",
    "no_test",
    "too_large",
];

pub fn compare(base: &Path, current: &Path) -> Result<Json, String> {
    let b = read_json(&base.join("pipeline.report.json"))?;
    let c = read_json(&current.join("pipeline.report.json"))?;
    let manifest_changed = hash_of(base) != hash_of(current);
    let (added, removed) = paths_diff(&base.join("accepted.jsonl"), &current.join("accepted.jsonl"))?;
    Ok(Json::obj()
        .put("manifest_changed", manifest_changed)
        .put("decisions_delta", delta(&b, &c, "decisions", DECISIONS))
        .put("reasons_delta", delta(&b, &c, "reasons", REASONS))
        .put(
            "accepted_added",
            Json::arr(added.iter().take(50).map(Json::str).collect()),
        )
        .put(
            "accepted_removed",
            Json::arr(removed.iter().take(50).map(Json::str).collect()),
        )
        .put("accepted_added_total", added.len() as u64)
        .put("accepted_removed_total", removed.len() as u64))
}

fn hash_of(dir: &Path) -> Option<String> {
    read_json(&dir.join("manifest.json"))
        .ok()
        .and_then(|j| j.get_str("hash").map(|s| s.to_string()))
}

fn delta(b: &Json, c: &Json, section: &str, keys: &[&str]) -> Json {
    let mut out = Json::obj();
    for k in keys {
        let bv = count(b, section, k);
        let cv = count(c, section, k);
        out = out.put((*k).to_string(), Json::num(cv - bv));
    }
    out
}

fn count(j: &Json, section: &str, key: &str) -> i64 {
    j.get(section)
        .and_then(|s| s.get(key))
        .and_then(Json::as_u64)
        .unwrap_or(0) as i64
}

fn paths_diff(base: &Path, current: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let bp = read_paths(base)?;
    let cp = read_paths(current)?;
    let added = cp.difference(&bp).cloned().collect();
    let removed = bp.difference(&cp).cloned().collect();
    Ok((added, removed))
}

fn read_paths(p: &Path) -> Result<BTreeSet<String>, String> {
    let text = match std::fs::read_to_string(p) {
        Ok(t) => t,
        Err(_) => return Ok(BTreeSet::new()),
    };
    let mut out = BTreeSet::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(j) = Json::parse(line)
            && let Some(path) = j.get_str("path")
        {
            out.insert(path.to_string());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("trit-diff-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn reports_file_changes_and_deltas() {
        let root = tmp("changes");
        let base = root.join("base");
        let current = root.join("current");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&current).unwrap();
        write(
            &base,
            "pipeline.report.json",
            r#"{"decisions":{"accept":2,"reject":1},"reasons":{"generated":1}}"#,
        );
        write(
            &current,
            "pipeline.report.json",
            r#"{"decisions":{"accept":1,"defer":1},"reasons":{"no_test":1}}"#,
        );
        write(&base, "manifest.json", r#"{"hash":"a"}"#);
        write(&current, "manifest.json", r#"{"hash":"b"}"#);
        write(&base, "accepted.jsonl", "{\"path\":\"a.rs\"}\n{\"path\":\"b.rs\"}\n");
        write(&current, "accepted.jsonl", "{\"path\":\"a.rs\"}\n{\"path\":\"c.rs\"}\n");
        let j = compare(&base, &current).unwrap();
        assert_eq!(j.get("manifest_changed").and_then(Json::as_bool), Some(true));
        assert!(j.dump().contains("\"accept\": -1"));
        assert_eq!(j.get("accepted_added_total").and_then(Json::as_u64), Some(1));
        assert_eq!(j.get("accepted_removed_total").and_then(Json::as_u64), Some(1));
        let _ = std::fs::remove_dir_all(&root);
    }
}
