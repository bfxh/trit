//! `repo` 子命令：扫描本地源码树，输出 RepoUnit、依赖图、去重、上下文与闭包。

use trit_data::repo;

use super::{die, repo_context, repo_opts, repo_out, repo_show, repo_stats, usage};

pub fn run(argv: &[String]) {
    let o = repo_opts::parse(argv);
    let root = o.root.clone().unwrap_or_else(|| usage());
    let units = repo::scan(&root).unwrap_or_else(|e| die(&e));
    let s = repo_stats::stats(&units);
    let d = repo::deps::summarize(&units);
    let dup = repo::dup::scan(&root, &units);
    println!("{}", repo_show::summary(&root, &units, &s, &d, &dup, o.closure.as_deref(), o.max_files));
    if let Some(q) = &o.context {
        repo_context::print_and_write(&root, &units, q, o.out.as_deref()).unwrap_or_else(|e| die(&e));
    }
    if let Some(dir) = o.out {
        repo_out::write(&dir, &root.display().to_string(), &units, &s, &d, &dup, o.closure.as_deref(), o.max_files);
    }
}
