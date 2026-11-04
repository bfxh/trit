//! 块级传输命令：`export` / `import`（TVBD 包）。

use std::path::PathBuf;
use std::process::ExitCode;

use trit_vcs::bundle;
use trit_vcs::repo::Repo;

use super::{fail, flag, positionals, repo_path};

pub(crate) fn export_cmd(args: &[String]) -> ExitCode {
    let root = repo_path(args);
    let repo = match Repo::open(&root) {
        Ok(repo) => repo,
        Err(err) => return fail(err.to_string()),
    };
    let ids = positionals(args, &["--repo", "--out", "--dest", "--prefix"]);
    let Some(target) = ids.first() else {
        eprintln!("export 需要树 id");
        return ExitCode::from(2);
    };
    let Some(tree) = trit_vcs::Hash::from_hex(target) else {
        return fail(format!("不是合法树 id：{target}"));
    };
    let Some(out_path) = flag(args, "--out") else {
        eprintln!("export 需要 --out FILE");
        return ExitCode::from(2);
    };
    let dest = match flag(args, "--dest") {
        Some(path) => match Repo::open(path) {
            Ok(repo) => Some(repo),
            Err(err) => return fail(err.to_string()),
        },
        None => None,
    };
    let mut file = match std::fs::File::create(PathBuf::from(out_path)) {
        Ok(file) => file,
        Err(err) => return fail(format!("{out_path}: {err}")),
    };
    let prefix = flag(args, "--prefix");
    match bundle::export(
        repo.store(),
        tree,
        prefix,
        dest.as_ref().map(|repo| repo.store()),
        &mut file,
    ) {
        Ok(stats) => {
            println!(
                "已导出 {}（{} 个文件，对象 {}，实传 {}/{}，字节 {}/{}）",
                out_path,
                stats.files,
                stats.objects,
                stats.sent_objects,
                stats.objects,
                stats.sent_bytes,
                stats.total_bytes
            );
            if stats.total_bytes > 0 {
                println!(
                    "  传输占比 {:.1}%（前缀 {:?}）",
                    stats.sent_bytes as f64 / stats.total_bytes as f64 * 100.0,
                    prefix.unwrap_or("全量")
                );
            }
            ExitCode::SUCCESS
        }
        Err(err) => fail(err.to_string()),
    }
}

pub(crate) fn import_cmd(args: &[String]) -> ExitCode {
    let root = repo_path(args);
    let repo = match Repo::open(&root) {
        Ok(repo) => repo,
        Err(err) => return fail(err.to_string()),
    };
    let ids = positionals(args, &["--repo"]);
    let Some(path) = ids.first() else {
        eprintln!("import 需要包文件");
        return ExitCode::from(2);
    };
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => return fail(format!("{path}: {err}")),
    };
    match bundle::import(repo.store(), &mut file) {
        Ok(stats) => {
            println!(
                "已导入：收到 {} 个对象（{} 字节），跳过已有 {} 个，树 {}",
                stats.received,
                stats.bytes,
                stats.skipped,
                stats
                    .tree
                    .map(|tree| tree.short())
                    .unwrap_or_else(|| "-".to_string())
            );
            ExitCode::SUCCESS
        }
        Err(err) => fail(err.to_string()),
    }
}
