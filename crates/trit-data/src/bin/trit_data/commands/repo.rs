//! `repo` 子命令：扫描本地源码树，输出 RepoUnit、依赖图与依赖闭包。

use trit_data::repo;
use trit_learn::json::write_json;

use super::{die, repo_opts, repo_stats, usage, write_lines};

pub fn run(argv: &[String]) {
    let o = repo_opts::parse(argv);
    let root = o.root.clone().unwrap_or_else(|| usage());
    let units = repo::scan(&root).unwrap_or_else(|e| die(&e));
    let s = repo_stats::stats(&units);
    let d = repo::deps::summarize(&units);
    println!(
        "仓库 {}：文件 {}（生成 {}，测试 {}，依赖边 {}，测试关联 {}，环上 {}）",
        root.display(),
        units.len(),
        s.generated,
        s.tests,
        d.edges,
        d.test_links,
        d.cyclic
    );
    if let Some(start) = &o.closure {
        let json = repo_stats::closure_json(&units, start, o.max_files);
        println!("闭包 {}：{} 个文件", start, json.dump());
    }
    if let Some(dir) = o.out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        write_lines(&dir.join("repo.units.jsonl"), &repo_stats::units_lines(&units));
        let root_s = root.display().to_string();
        let deps = repo_stats::deps_document(&root_s, &units, &d);
        write_json(&dir.join("repo.deps.json"), &deps).unwrap_or_else(|e| die(&e));
        let report = repo_stats::report(&root_s, &units, &s, &d);
        write_json(&dir.join("repo.report.json"), &report).unwrap_or_else(|e| die(&e));
        if let Some(start) = &o.closure {
            let c = repo_stats::closure_json(&units, start, o.max_files);
            write_json(&dir.join("repo.closure.json"), &c).unwrap_or_else(|e| die(&e));
        }
    }
}
