//! crate 级测试覆盖单测：内联测试与 `tests/` 都能覆盖同 crate 的文件。

use super::crates::annotate;
use super::deps_tests::unit;

#[test]
fn crate_with_tests_covers_its_files() {
    let mut units = vec![
        unit("crates/a/src/lib.rs", "rust", &[], false),
        unit("crates/a/src/foo.rs", "rust", &[], false),
        unit("crates/a/Cargo.toml", "toml", &[], false),
        unit("crates/b/src/lib.rs", "rust", &[], false),
        unit("crates/b/Cargo.toml", "toml", &[], false),
        unit("crates/b/tests/it.rs", "rust", &[], true),
        unit("crates/c/src/lib.rs", "rust", &[], false),
        unit("crates/c/Cargo.toml", "toml", &[], false),
    ];
    units[1].inline_tests = true;
    annotate(&mut units);
    assert!(units[0].crate_tests && units[1].crate_tests && units[2].crate_tests);
    assert!(units[3].crate_tests && units[5].crate_tests);
    assert!(!units[6].crate_tests && !units[7].crate_tests);
}
