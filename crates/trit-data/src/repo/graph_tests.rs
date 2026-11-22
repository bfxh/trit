//! 依赖图单测：闭包与环计数。

use super::deps::{annotate, closure, summarize};
use super::deps_tests::unit;

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

