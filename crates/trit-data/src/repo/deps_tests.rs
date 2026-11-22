//! repo_deps / repo_imports 单测。

use super::deps::{annotate, summarize};
use super::Unit;

pub fn unit(path: &str, lang: &str, imports: &[&str], is_test: bool) -> Unit {
    Unit {
        repo: "r".into(),
        path: path.into(),
        lang: lang.into(),
        imports: imports.iter().map(|s| s.to_string()).collect(),
        imports_known: true,
        is_test,
        ..Default::default()
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
fn external_and_local_unresolved_are_split() {
    let mut units = vec![unit(
        "src/a.rs",
        "rust",
        &["std::io", "serde::Serialize", "crate::missing::Thing"],
        false,
    )];
    annotate(&mut units);
    assert!(units[0].imports_resolved.is_empty());
    assert_eq!(
        units[0].imports_external,
        vec!["std::io".to_string(), "serde::Serialize".to_string()]
    );
    assert_eq!(
        units[0].unresolved,
        vec!["crate::missing::Thing".to_string()]
    );
}
