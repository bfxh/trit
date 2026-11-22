//! repo_deps / repo_imports 单测。

use super::deps::{annotate, closure, summarize};
use super::Unit;

fn unit(path: &str, lang: &str, imports: &[&str], is_test: bool) -> Unit {
    Unit {
        repo: "r".into(),
        path: path.into(),
        lang: lang.into(),
        bytes: 0,
        generated: false,
        license: None,
        imports: imports.iter().map(|s| s.to_string()).collect(),
        imports_known: true,
        imports_resolved: Vec::new(),
        unresolved: Vec::new(),
        tests: Vec::new(),
        tests_for: Vec::new(),
        is_test,
    }
}

#[test]
fn js_relative_imports_resolve_and_tests_link() {
    let mut units = vec![
        unit("web/src/a.ts", "typescript", &["./b", "../lib/c"], false),
        unit("web/src/b.ts", "typescript", &[], false),
        unit("web/lib/c.ts", "typescript", &[], false),
        unit("web/src/a.test.ts", "typescript", &["./a"], true),
    ];
    annotate(&mut units);
    assert_eq!(
        units[0].imports_resolved,
        vec!["web/src/b.ts".to_string(), "web/lib/c.ts".to_string()]
    );
    assert_eq!(units[3].tests_for, vec!["web/src/a.ts".to_string()]);
    assert_eq!(units[0].tests, vec!["web/src/a.test.ts".to_string()]);
    let s = summarize(&units);
    assert_eq!((s.edges, s.tested_sources, s.test_links), (3, 1, 1));
}

#[test]
fn python_absolute_and_relative_imports_resolve() {
    let mut units = vec![
        unit("pkg/app.py", "python", &["pkg.util", ".helper"], false),
        unit("pkg/util.py", "python", &[], false),
        unit("pkg/helper.py", "python", &[], false),
        unit("pkg/test_app.py", "python", &["pkg.app"], true),
    ];
    annotate(&mut units);
    assert_eq!(
        units[0].imports_resolved,
        vec!["pkg/util.py".to_string(), "pkg/helper.py".to_string()]
    );
    assert_eq!(units[3].tests_for, vec!["pkg/app.py".to_string()]);
}

#[test]
fn rust_mods_and_crate_paths_use_the_crate_prefix() {
    let mut units = vec![
        unit("crates/x/src/lib.rs", "rust", &["a", "crate::b::Thing"], false),
        unit("crates/x/src/a.rs", "rust", &[], false),
        unit("crates/x/src/b.rs", "rust", &[], false),
        unit("crates/x/tests/it.rs", "rust", &["crate::b::Thing"], true),
    ];
    annotate(&mut units);
    assert_eq!(
        units[0].imports_resolved,
        vec!["crates/x/src/a.rs".to_string(), "crates/x/src/b.rs".to_string()]
    );
    assert_eq!(units[3].tests_for, vec!["crates/x/src/lib.rs".to_string()]);
}

#[test]
fn external_imports_are_unresolved_not_dropped() {
    let mut units = vec![unit("src/a.rs", "rust", &["std::io", "serde::Serialize"], false)];
    annotate(&mut units);
    assert!(units[0].imports_resolved.is_empty());
    assert_eq!(units[0].unresolved, vec!["std::io".to_string(), "serde::Serialize".to_string()]);
}

#[test]
fn closure_follows_edges_and_honours_the_cap() {
    let mut units = vec![
        unit("a.ts", "typescript", &["./b"], false),
        unit("b.ts", "typescript", &["./c"], false),
        unit("c.ts", "typescript", &[], false),
    ];
    annotate(&mut units);
    assert_eq!(
        closure(&units, "a.ts", 10),
        vec!["a.ts".to_string(), "b.ts".to_string(), "c.ts".to_string()]
    );
    assert_eq!(closure(&units, "a.ts", 2), vec!["a.ts".to_string(), "b.ts".to_string()]);
}

#[test]
fn cycles_are_counted_after_kahn() {
    let mut units = vec![
        unit("a.ts", "typescript", &["./b"], false),
        unit("b.ts", "typescript", &["./a"], false),
    ];
    annotate(&mut units);
    assert_eq!(summarize(&units).cyclic, 2);
}

#[test]
fn parses_rust_mod_and_use_declarations() {
    let (v, known) = super::imports::parse("pub mod a;
mod b;
use crate::c::{D, E};
mod tests {", "rust");
    assert!(known);
    assert_eq!(v, vec!["a", "b", "crate::c::D", "crate::c::E"]);
}
