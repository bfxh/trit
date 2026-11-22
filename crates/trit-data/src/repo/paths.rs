//! 仓库内路径工具：归一化、上跳、文件名，以及测试文件 → 被测文件的目标匹配。

use std::collections::BTreeSet;

use crate::repo::Unit;

pub const JS_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "json"];

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
    if u.lang == "rust" {
        cands.extend(rust_targets(u, &base));
    }
    cands.dedup();
    cands.retain(|p| p != &u.path && paths.contains(p));
    cands
}

fn rust_targets(u: &Unit, base: &str) -> Vec<String> {
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

pub fn parent_dir(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

pub fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
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

pub fn up(dir: &str, n: usize) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for _ in 0..n {
        parts.pop();
    }
    parts.join("/")
}

pub fn join(dir: &str, tail: &str) -> String {
    norm(&format!("{dir}/{tail}"))
}

pub fn norm(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}
