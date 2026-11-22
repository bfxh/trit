//! 同目录唯一候选单测。

use std::collections::BTreeSet;

use super::deps_tests::unit;
use super::paths_unique::unique_sibling;

fn paths(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn unique_sibling_when_only_one_source() {
    let set = paths(&["pkg/Neo.java", "pkg/NeoTest.java", "pkg/README.md"]);
    let test = unit("pkg/NeoTest.java", "java", &[], true);
    assert_eq!(unique_sibling(&test, &set), Some("pkg/Neo.java".to_string()));
}

#[test]
fn multiple_sources_are_not_guessed() {
    let set = paths(&["pkg/A.java", "pkg/B.java", "pkg/NeoTest.java"]);
    let test = unit("pkg/NeoTest.java", "java", &[], true);
    assert_eq!(unique_sibling(&test, &set), None);
}
