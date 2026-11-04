//! 查询命令：`where` / `tests` / `explain`。
//!
//! 只读 techmap 索引，不修改它；索引路径来自 `--techmap` 或 `TRIT_TECHMAP`。

use std::collections::BTreeSet;
use std::process::ExitCode;

use trit_vcs::repo::Repo;
use trit_vcs::techmap::{Techmap, test_command};

use super::{fail, flag, has, positionals, repo_path};

pub(crate) fn load_techmap(args: &[String]) -> Result<Techmap, String> {
    let path = flag(args, "--techmap")
        .map(|value| value.to_string())
        .or_else(|| std::env::var("TRIT_TECHMAP").ok())
        .ok_or_else(|| "需要 --techmap FILE（或环境变量 TRIT_TECHMAP）".to_string())?;
    Techmap::load(&path).map_err(|err| format!("{path}: {err}"))
}

pub(crate) fn where_cmd(args: &[String]) -> ExitCode {
    let ids = positionals(args, &["--repo", "--techmap"]);
    let Some(needle) = ids.first() else {
        eprintln!("where 需要一个路径/测试名/标签");
        return ExitCode::from(2);
    };
    let map = match load_techmap(args) {
        Ok(map) => map,
        Err(err) => return fail(err),
    };
    let hits = map.where_entries(needle);
    if hits.is_empty() {
        println!("没有命中：{needle}");
        return ExitCode::FAILURE;
    }
    if has(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&hits).unwrap_or_default()
        );
    } else {
        for entry in &hits {
            println!("{} [{}] {}", entry.id, entry.status, entry.title);
            if !entry.paths.is_empty() {
                println!("    paths: {}", entry.paths.join(", "));
            }
            if !entry.tests.is_empty() {
                println!("    tests: {}", entry.tests.join(", "));
            }
        }
    }
    ExitCode::SUCCESS
}

pub(crate) fn tests_cmd(args: &[String]) -> ExitCode {
    let ids = positionals(args, &["--repo", "--techmap"]);
    let Some(needle) = ids.first() else {
        eprintln!("tests 需要一个路径/测试名/标签");
        return ExitCode::from(2);
    };
    let map = match load_techmap(args) {
        Ok(map) => map,
        Err(err) => return fail(err),
    };
    let mut seen = BTreeSet::new();
    for entry in map.where_entries(needle) {
        for spec in &entry.tests {
            if seen.insert(spec.clone()) {
                println!("{}", test_command(spec).unwrap_or_else(|| spec.clone()));
            }
        }
    }
    if seen.is_empty() {
        println!("这些条目没有登记测试：{needle}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

pub(crate) fn explain_cmd(args: &[String]) -> ExitCode {
    let root = repo_path(args);
    let repo = match Repo::open(&root) {
        Ok(repo) => repo,
        Err(err) => return fail(err.to_string()),
    };
    let ids = positionals(args, &["--repo", "--techmap"]);
    let Some(target) = ids.first() else {
        eprintln!("explain 需要提交 id");
        return ExitCode::from(2);
    };
    let Some(id) = trit_vcs::Hash::from_hex(target) else {
        return fail(format!("不是合法提交 id：{target}"));
    };
    let commit = match repo.commit_at(&id) {
        Ok(commit) => commit,
        Err(err) => return fail(err.to_string()),
    };
    let map = if flag(args, "--techmap").is_some() || std::env::var("TRIT_TECHMAP").is_ok() {
        match load_techmap(args) {
            Ok(map) => Some(map),
            Err(err) => return fail(err),
        }
    } else {
        None
    };
    let entries = map
        .as_ref()
        .map(|map| map.for_tags(&commit.tags))
        .unwrap_or_default();
    let mut tests = BTreeSet::new();
    for entry in &entries {
        for spec in &entry.tests {
            tests.insert(test_command(spec).unwrap_or_else(|| spec.clone()));
        }
    }
    if has(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": id.to_hex(),
                "tree": commit.tree.to_hex(),
                "session": commit.session,
                "claims": commit.claims,
                "tags": commit.tags,
                "evidence": commit.evidence,
                "message": commit.message,
                "witness": commit.witness.map(|witness| witness.to_hex()),
                "intent": commit.intent.as_ref().map(|intent| serde_json::json!({
                    "goal": intent.goal,
                    "accept": intent.accept,
                    "base": intent.base.map(|base| base.to_hex()),
                })),
                "entries": entries,
                "tests": tests.into_iter().collect::<Vec<_>>(),
            }))
            .unwrap_or_default()
        );
    } else {
        println!("提交 {}  树 {}", id.short(), commit.tree.short());
        println!("  会话 {}", commit.session);
        if !commit.tags.is_empty() {
            println!("  标签 {}", commit.tags.join(", "));
        }
        if !commit.evidence.is_empty() {
            println!("  证据 {}", commit.evidence.join(" | "));
        }
        if let Some(witness) = commit.witness {
            println!("  见证 {}", witness.short());
        }
        if let Some(intent) = &commit.intent {
            println!(
                "  意图 {}（accept {} 条，base {}）",
                intent.goal,
                intent.accept.len(),
                intent
                    .base
                    .map(|base| base.short())
                    .unwrap_or_else(|| "-".to_string())
            );
        }
        for entry in &entries {
            println!(
                "  技术 {} [{}] {}（{} 个测试）",
                entry.id,
                entry.status,
                entry.title,
                entry.tests.len()
            );
        }
        for command in &tests {
            println!("  {command}");
        }
    }
    ExitCode::SUCCESS
}
