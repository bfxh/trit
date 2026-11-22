//! `repo` 子命令：扫描本地源码树，输出 RepoUnit、依赖图、去重与依赖闭包。

use trit_data::repo;

use super::{die, repo_opts, repo_out, repo_show, repo_stats, usage};

pub fn run(argv: &[String]) {
    let o = repo_opts::parse(argv);
    let root = o.root.clone().unwrap_or_else(|| usage());
    let units = repo::scan(&root).unwrap_or_else(|e| die(&e));
    let s = repo_stats::stats(&units);
    let d = repo::deps::summarize(&units);
    let dup = repo::dup::scan(&root, &units);
    let closure = o.closure.as_deref();
    println!("{}", repo_show::summary(&root, &units, &s, &d, &dup, closure, o.max_files));
    if let Some(dir) = o.out {
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| die(&format!("建不了 {}: {e}", dir.display())));
        repo_out::write(&dir, &root.display().to_string(), &units, &s, &d, &dup, closure, o.max_files);
    }
}
