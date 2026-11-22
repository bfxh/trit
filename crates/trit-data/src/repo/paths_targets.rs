//! 测试文件 → 被测文件的目标候选：JS/TS、Python、Go、Rust、JVM 两类约定。

use std::collections::BTreeSet;

use super::paths::{JS_EXTS, join, parent_dir, up};
use super::paths_rules::{jvm_targets, rust_targets, stem_of, strip_test_marks};
use super::paths_unique::unique_sibling;
use crate::repo::Unit;
pub fn targets(u: &Unit, paths: &BTreeSet<String>) -> Vec<String> {
    let dir = parent_dir(&u.path);
    let base = strip_test_marks(&stem_of(&u.path));
    let exts: &[&str] = match u.lang.as_str() {
        "python" => &["py"],
        "go" => &["go"],
        "javascript" | "typescript" => JS_EXTS,
        _ => &[],
    };
    let mut cands = Vec::new();
    for e in exts {
        cands.push(join(&dir, &format!("{base}.{e}")));
        cands.push(join(&up(&dir, 1), &format!("{base}.{e}")));
    }
    if matches!(u.lang.as_str(), "java" | "kotlin" | "scala") {
        cands.extend(jvm_targets(u));
    }
    if u.lang == "rust" {
        cands.extend(rust_targets(u, &base));
    }
    cands.dedup();
    cands.retain(|p| p != &u.path && paths.contains(p));
    if cands.is_empty()
        && let Some(one) = unique_sibling(u, paths)
    {
        cands.push(one);
    }
    cands
}
