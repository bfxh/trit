//! 中央仓同步命令：`publish`（把提交写成中央仓的命名 ref）/ `pull`（按需物化，可只拉一个目录）。

use std::process::ExitCode;

use trit_vcs::central;
use trit_vcs::repo::Repo;

use super::{fail, flag, has, positionals, repo_path};

/// 两侧都允许"目录还不存在"：`init` 对已有仓是幂等的，等于 open-or-create。
fn repo_at(path: impl Into<std::path::PathBuf>) -> Result<Repo, ExitCode> {
    Repo::init(path).map_err(|err| fail(err.to_string()))
}

pub(crate) fn publish_cmd(args: &[String]) -> ExitCode {
    let ids = positionals(args, &["--repo", "--central", "--commit"]);
    let Some(name) = ids.first() else {
        eprintln!("publish 需要 ref 名：trit-vcs publish main --central PATH");
        return ExitCode::from(2);
    };
    let Some(central_path) = flag(args, "--central") else {
        eprintln!("publish 需要 --central PATH");
        return ExitCode::from(2);
    };
    let client = match repo_at(repo_path(args)) {
        Ok(repo) => repo,
        Err(code) => return code,
    };
    let mut central_repo = match repo_at(central_path) {
        Ok(repo) => repo,
        Err(code) => return code,
    };
    let commit = match flag(args, "--commit") {
        Some(hex) => match trit_vcs::Hash::from_hex(hex) {
            Some(id) => Some(id),
            None => return fail(format!("不是合法提交 id：{hex}")),
        },
        None => None,
    };
    match central::publish(
        &client,
        &mut central_repo,
        name,
        commit.as_ref(),
        has(args, "--force"),
    ) {
        Ok(stats) => {
            println!(
                "已发布 {} → 中央仓 ref {name}（对象 {}/{} 实传，字节 {}/{}）",
                stats.commit.short(),
                stats.transfer.sent_objects,
                stats.transfer.objects,
                stats.transfer.sent_bytes,
                stats.transfer.total_bytes
            );
            ExitCode::SUCCESS
        }
        Err(err) => fail(err.to_string()),
    }
}

pub(crate) fn pull_cmd(args: &[String]) -> ExitCode {
    let ids = positionals(args, &["--repo", "--central", "--prefix"]);
    let Some(name) = ids.first() else {
        eprintln!("pull 需要 ref 名：trit-vcs pull main --central PATH [--prefix DIR]");
        return ExitCode::from(2);
    };
    let Some(central_path) = flag(args, "--central") else {
        eprintln!("pull 需要 --central PATH");
        return ExitCode::from(2);
    };
    let mut client = match repo_at(repo_path(args)) {
        Ok(repo) => repo,
        Err(code) => return code,
    };
    let central_repo = match Repo::open(central_path) {
        Ok(repo) => repo,
        Err(err) => return fail(err.to_string()),
    };
    let prefix = flag(args, "--prefix");
    match central::pull(
        &central_repo,
        &mut client,
        name,
        prefix,
        has(args, "--force"),
    ) {
        Ok(stats) => {
            println!(
                "已拉取 ref {name}（提交 {}）：对象 {}/{} 实传，字节 {}/{}，物化 {} 个文件",
                stats.commit.short(),
                stats.transfer.sent_objects,
                stats.transfer.objects,
                stats.transfer.sent_bytes,
                stats.transfer.total_bytes,
                stats.written
            );
            if stats.skipped > 0 {
                println!("  范围外跳过 {} 个（前缀 {:?}）", stats.skipped, prefix);
            }
            ExitCode::SUCCESS
        }
        Err(err) => fail(err.to_string()),
    }
}
