//! `repo` 子命令：扫描本地源码树，输出 RepoUnit。

use std::collections::BTreeMap;
use std::path::PathBuf;

use trit_data::repo::scan;
use trit_learn::json::{Json, write_json};

use super::{die, usage, write_lines};

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
    let mut lines = Vec::new();
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
        lines.push(
            Json::obj()
                .put("repo", u.repo.clone())
                .put("path", u.path.clone())
                .put("lang", u.lang.clone())
                .put("bytes", u.bytes)
                .put("generated", u.generated)
                .put(
                    "license",
                    u.license.clone().unwrap_or_else(|| "missing".into()),
                )
                .put(
                    "imports",
                    Json::arr(u.imports.iter().map(Json::str).collect()),
                )
                .put("is_test", u.is_test)
                .dump(),
        );
    }
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
        let mut lang_json = Json::obj();
        for (k, v) in &langs {
            lang_json = lang_json.put(k.clone(), *v as u64);
        }
        let mut lic_json = Json::obj();
        for (k, v) in &licenses {
            lic_json = lic_json.put(k.clone(), *v as u64);
        }
        let report = Json::obj()
            .put("schema", 1u32)
            .put("kind", "trit-data-repo")
            .put("repo", root.display().to_string())
            .put("files", units.len() as u64)
            .put("languages", lang_json)
            .put("licenses", lic_json)
            .put("generated", generated as u64)
            .put("tests", tests as u64)
            .put("with_imports", with_imports as u64);
        write_json(&dir.join("repo.report.json"), &report).unwrap_or_else(|e| die(&e));
    }
}
