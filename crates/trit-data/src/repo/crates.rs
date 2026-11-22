//! crate 级测试覆盖：同 `Cargo.toml` 的 crate 内任意文件有测试（内联或 `tests/`），
//! 则该 crate 的所有文件都视为"被 crate 级测试覆盖"。
//!
//! 这是**弱**覆盖（文件级覆盖见 `Unit::tests` / `Unit::inline_tests`），只用于
//! `no_test` 软门，避免把"crate 有集成测试但该文件没有同名测试"当成无测试。

use std::collections::BTreeSet;

use super::paths::parent_dir;
use crate::repo::Unit;

pub fn annotate(units: &mut [Unit]) {
    let paths: BTreeSet<String> = units.iter().map(|u| u.path.clone()).collect();
    let mut with_tests: BTreeSet<String> = BTreeSet::new();
    for u in units.iter().filter(|u| u.is_test || u.inline_tests) {
        if let Some(root) = crate_root(&u.path, &paths) {
            with_tests.insert(root);
        }
    }
    for u in units.iter_mut() {
        u.crate_tests = crate_root(&u.path, &paths).is_some_and(|r| with_tests.contains(&r));
    }
}

/// 最近的含 `Cargo.toml` 的祖先目录（空串表示仓库根）。
fn crate_root(path: &str, paths: &BTreeSet<String>) -> Option<String> {
    let mut dir = parent_dir(path);
    loop {
        let marker = if dir.is_empty() {
            "Cargo.toml".to_string()
        } else {
            format!("{dir}/Cargo.toml")
        };
        if paths.contains(&marker) {
            return Some(dir);
        }
        if dir.is_empty() {
            return None;
        }
        dir = parent_dir(&dir);
    }
}
