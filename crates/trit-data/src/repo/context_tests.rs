//! 检索上下文单测：BM25 命中、依赖闭包补入、token 预算。

use std::path::{Path, PathBuf};

use super::context::bundle;
use super::deps_tests::unit;

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("trit-ctx-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write(root: &Path, rel: &str, body: &str) {
    std::fs::write(root.join(rel), body).unwrap();
}

#[test]
fn bm25_hits_and_dependency_closure_fill_the_bundle() {
    let root = tmp("bundle");
    write(&root, "a.rs", "fn quantize(v: i64) -> i64 { v }\n");
    write(&root, "b.rs", "fn scale(x: i64) -> i64 { x * 3 }\n");
    write(&root, "c.rs", "fn unrelated() -> &'static str { \"nothing here\" }\n");
    let mut a = unit("a.rs", "rust", &[], false);
    a.imports_resolved = vec!["b.rs".to_string()];
    let units = vec![a, unit("b.rs", "rust", &[], false), unit("c.rs", "rust", &[], false)];
    let b = bundle(&root, &units, "quantize", 10_000, 8);
    assert_eq!(b.hits[0].path, "a.rs");
    assert_eq!(b.hits[0].reason, "hit");
    assert!(b.hits.iter().any(|h| h.path == "b.rs" && h.reason == "dep"));
    assert!(!b.hits.iter().any(|h| h.path == "c.rs"));
    assert!(!b.sha256.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn budget_limits_the_bundle() {
    let root = tmp("budget");
    write(&root, "a.rs", "fn quantize(v: i64) -> i64 { v }\n");
    write(&root, "b.rs", "fn scale(x: i64) -> i64 { x * 3 }\n");
    let mut a = unit("a.rs", "rust", &[], false);
    a.imports_resolved = vec!["b.rs".to_string()];
    let units = vec![a, unit("b.rs", "rust", &[], false)];
    let b = bundle(&root, &units, "quantize", 1, 8);
    assert_eq!(b.hits.len(), 1);
    assert_eq!(b.hits[0].path, "a.rs");
    let _ = std::fs::remove_dir_all(&root);
}
