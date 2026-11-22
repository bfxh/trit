//! 测试文件 → 被测文件的目标候选：JS/TS、Python、Go、Rust、JVM 两类约定。

use std::collections::BTreeSet;

use super::paths::{JS_EXTS, file_name, join, parent_dir, up};
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
    cands
}fn rust_targets(u: &Unit, base: &str) -> Vec<String> {
    let prefix = u
        .path
        .find("src/")
        .or_else(|| u.path.find("tests/"))
        .map(|i| &u.path[..i])
        .unwrap_or("");
    let dir = parent_dir(&u.path);
    vec![
        join(&dir, &format!("{base}.rs")),
        join(&dir, &format!("{base}/mod.rs")),
        format!("{prefix}src/{base}.rs"),
        format!("{prefix}src/{base}/mod.rs"),
        format!("{prefix}src/lib.rs"),
        format!("{prefix}src/main.rs"),
    ]
}
fn jvm_targets(u: &Unit) -> Vec<String> {
    let stem = stem_of(&u.path);
    let base = strip_jvm_marks(&stem);
    if base.is_empty() || base == stem {
        return Vec::new();
    }
    let dir = parent_dir(&u.path);
    let main = jvm_main_dir(&dir);
    let mut cands = Vec::new();
    for d in [main.as_str(), dir.as_str()] {
        for e in ["java", "kt", "scala"] {
            cands.push(join(d, &format!("{base}.{e}")));
        }
    }
    cands
}

/// `FooTest` / `FooTests` / `FooIT` / `FooITCase` / `FooSpec` → `Foo`。
fn strip_jvm_marks(stem: &str) -> String {
    for s in ["ITCase", "Tests", "Test", "IT", "Spec"] {
        if let Some(b) = stem.strip_suffix(s)
            && !b.is_empty()
        {
            return b.to_string();
        }
    }
    stem.to_string()
}

/// `src/test/java` → `src/main/java`（kotlin / scala 同理）。
fn jvm_main_dir(dir: &str) -> String {
    for (from, to) in [
        ("src/test/java", "src/main/java"),
        ("src/test/kotlin", "src/main/kotlin"),
        ("src/test/scala", "src/main/scala"),
        ("src/test", "src/main"),
    ] {
        if let Some(i) = dir.find(from) {
            return format!("{}{to}{}", &dir[..i], &dir[i + from.len()..]);
        }
    }
    dir.to_string()
}
fn stem_of(p: &str) -> String {
    let n = file_name(p);
    match n.rfind('.') {
        Some(i) => n[..i].to_string(),
        None => n.to_string(),
    }
}
fn strip_test_marks(stem: &str) -> String {
    let s = strip_pair(stem, "test_", "tests_");
    let s = strip_pair(s, "_test", "_tests");
    let s = s.strip_suffix(".test").unwrap_or(s);
    s.strip_suffix(".spec").unwrap_or(s).to_string()
}
fn strip_pair<'a>(s: &'a str, a: &str, b: &str) -> &'a str {
    s.strip_prefix(a)
        .or_else(|| s.strip_prefix(b))
        .unwrap_or(s)
}