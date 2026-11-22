//! 去重单测：精确重复与近似重复。

use std::path::{Path, PathBuf};

use super::deps_tests::unit;
use super::dup::{THRESHOLD, scan};

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("trit-dup-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write(root: &Path, rel: &str, body: &str) {
    std::fs::write(root.join(rel), body).unwrap();
}

#[test]
fn exact_and_near_duplicates_are_grouped() {
    let root = tmp("group");
    let mut base = String::from("fn compute(value: i64) -> i64 {\n    let scale = 3;\n    value * scale + 1\n}\n");
    for i in 0..30 {
        base.push_str(&format!("// line {i} padding padding padding\n"));
    }
    write(&root, "a.rs", &base);
    write(&root, "b.rs", &base);
    write(&root, "c.rs", &format!("{base}\nfn extra() -> i64 {{ 42 }}\n"));
    let units: Vec<super::Unit> = vec![
        unit("a.rs", "rust", &[], false),
        unit("b.rs", "rust", &[], false),
        unit("c.rs", "rust", &[], false),
    ];
    let d = scan(&root, &units);
    assert_eq!(d.exact.len(), 1);
    assert_eq!(d.exact[0].paths, vec!["a.rs".to_string(), "b.rs".to_string()]);
    assert_eq!(d.near.len(), 1);
    assert!(d.near[0].similarity >= THRESHOLD);
    assert_eq!(d.near[0].paths, vec!["a.rs".to_string(), "c.rs".to_string()]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn small_and_test_files_are_skipped() {
    let root = tmp("skip");
    write(&root, "tiny.rs", "fn x() {}\n");
    let units = vec![
        unit("tiny.rs", "rust", &[], false),
        unit("a_test.rs", "rust", &[], true),
    ];
    let d = scan(&root, &units);
    assert!(d.exact.is_empty() && d.near.is_empty());
    assert_eq!(d.skipped, 2);
    let _ = std::fs::remove_dir_all(&root);
}
