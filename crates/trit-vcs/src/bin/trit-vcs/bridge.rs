//! Git 桥命令：`git-export` / `git-import`。

use std::path::Path;
use std::process::ExitCode;

use trit_vcs::git_bridge;
use trit_vcs::repo::Repo;

use super::{fail, flag, positionals, repo_path};

fn write_map(path: Option<&str>, rows: Vec<serde_json::Value>) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    let encoded = serde_json::to_vec_pretty(&rows).map_err(|err| err.to_string())?;
    std::fs::write(path, encoded).map_err(|err| format!("{path}: {err}"))
}

pub(crate) fn export_cmd(args: &[String]) -> ExitCode {
    let root = repo_path(args);
    let repo = match Repo::open(&root) {
        Ok(repo) => repo,
        Err(err) => return fail(err.to_string()),
    };
    let ids = positionals(args, &["--repo", "--map"]);
    let Some(target) = ids.first() else {
        eprintln!("git-export 需要目标 Git 目录");
        return ExitCode::from(2);
    };
    match git_bridge::export(&repo, Path::new(target)) {
        Ok(stats) => {
            println!(
                "已导出 {} 个提交到 {}（写 {} 个文件）",
                stats.commits, target, stats.files_written
            );
            let rows = stats
                .map
                .iter()
                .map(|(trit, git)| serde_json::json!({"trit": trit.to_hex(), "git": git}))
                .collect();
            if let Err(err) = write_map(flag(args, "--map"), rows) {
                return fail(err);
            }
            ExitCode::SUCCESS
        }
        Err(err) => fail(err.to_string()),
    }
}

pub(crate) fn import_cmd(args: &[String]) -> ExitCode {
    let root = repo_path(args);
    let mut repo = match Repo::open(&root) {
        Ok(repo) => repo,
        Err(err) => return fail(err.to_string()),
    };
    let ids = positionals(args, &["--repo", "--map"]);
    let Some(target) = ids.first() else {
        eprintln!("git-import 需要 Git 目录");
        return ExitCode::from(2);
    };
    match git_bridge::import(&mut repo, Path::new(target)) {
        Ok(stats) => {
            println!(
                "已导入 {} 个提交（{} 个文件条目）自 {}",
                stats.commits, stats.files, target
            );
            let rows = stats
                .map
                .iter()
                .map(|(git, trit)| serde_json::json!({"git": git, "trit": trit.to_hex()}))
                .collect();
            if let Err(err) = write_map(flag(args, "--map"), rows) {
                return fail(err);
            }
            ExitCode::SUCCESS
        }
        Err(err) => fail(err.to_string()),
    }
}
