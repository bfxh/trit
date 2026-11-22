//! `repo` 子命令：扫描本地源码树，输出 RepoUnit。

use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_data::repo::scan;
use trit_learn::json::write_json;

use super::{die, usage, write_lines};

#[path = "repo_units.rs"]
mod repo_units;

#[path = "repo_report.rs"]
mod repo_report;

pub fn run(argv: &[String]) {
    let mut root: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--repo" => {
                root = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                out = argv.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => die(&format!("未知参数 {other}")),
        }
    }
    let root = root.unwrap_or_else(|| usage());
    let units = scan(&root).unwrap_or_else(|e| die(&e));
    let mut langs: BTreeMap<String, usize> = BTreeMap::new();
    let mut licenses: BTreeMap<String, usize> = BTreeMap::new();
    let (mut generated, mut tests, mut with_imports) = (0usize, 0usize, 0usize);
    let (mut license_missing, mut imports_unknown) = (0usize, 0usize);
    for u in &units {
        *langs.entry(u.lang.clone()).or_insert(0) += 1;
        *licenses
            .entry(u.license.clone().unwrap_or_else(|| "missing".into()))
            .or_insert(0) += 1;
        if u.generated {
            generated += 1;
        }
        if u.is_test {
            tests += 1;
        }
        if !u.imports.is_empty() {
            with_imports += 1;
        }
        if u.license.is_none() {
            license_missing += 1;
        }
        if !u.imports_known {
            imports_unknown += 1;
        }
    }
    let lines = repo_units::lines(&units);
    println!(
        "仓库 {}：文件 {}（生成 {}，测试 {}，有依赖 {}）",
        root.display(),
        units.len(),
        generated,
        tests,
        with_imports
    );
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        write_lines(&dir.join("repo.units.jsonl"), &lines);
        let report = repo_report::build(&repo_report::Stats {
            repo: &root.display().to_string(),
            files: units.len(),
            languages: &langs,
            licenses: &licenses,
            generated,
            tests,
            with_imports,
            license_missing,
            imports_unknown,
        });
        write_json(&dir.join("repo.report.json"), &report).unwrap_or_else(|e| die(&e));
    }
}
