//! `trit-vcs` 命令行：init / add / commit / log / verify / stats。
//!
//! 默认行为就是产品立场：**没有在效认领覆盖的提交会被拒绝**；要放行必须显式
//! `--allow-unclaimed`，并会被记进哈希链（不是静默）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use trit_vcs::gate::{ClaimSpec, Verdict, load_claims};
use trit_vcs::merge;
use trit_vcs::model::Intent;
use trit_vcs::repo::CommitFilter;
use trit_vcs::repo::{CommitRequest, Repo};
use trit_vcs::witness::{RunOutput, Witness};
use trit_vcs::worktree;

mod bridge;
mod mcp_serve;
mod query;
mod transport;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn usage() -> ExitCode {
    eprintln!(
        "用法：\n  \
         trit-vcs init [--repo PATH]\n  \
         trit-vcs add <文件>... [--repo PATH]\n  \
         trit-vcs commit -m <说明> --owner <宿主:会话id> [--claims FILE] [--evidence 文本]... \n                   \
         [--tag 标签]... [--witness FILE] [--goal 目标] [--accept 判据]... [--allow-unclaimed] [--repo PATH]\n  \
         trit-vcs witness [--out FILE] [--repo PATH] -- <命令>...\n  \
         trit-vcs admit <提交id> --witness FILE [--repo PATH]\n  \
         trit-vcs export <树id> --out FILE [--prefix DIR] [--dest REPO] [--repo SRC]\n  \
         trit-vcs import <包文件> [--repo DEST]\n  \
         trit-vcs git-export <git目录> [--map FILE] [--repo PATH]\n  \
         trit-vcs mcp [--repo PATH]\n  \
         trit-vcs git-import <git目录> [--map FILE] [--repo PATH]\n  \
         trit-vcs merge <ours提交> <theirs提交> [--base <提交>] [--json] [--repo PATH]\n  \
         trit-vcs status [--prefix DIR] [--json] [--repo PATH]\n  \
         trit-vcs checkout <提交id> [--prefix DIR] [--force] [--repo PATH]\n  \
         trit-vcs log [--json] [--tag T] [--evidence 文本] [--session S] [--grep 文本] [--repo PATH]\n  \
         trit-vcs where <路径/测试/标签> --techmap FILE [--json]\n  \
         trit-vcs tests <路径/测试/标签> --techmap FILE\n  \
         trit-vcs explain <提交id> [--techmap FILE] [--json] [--repo PATH]\n  \
         trit-vcs verify [--repo PATH]\n  \
         trit-vcs stats [--repo PATH]"
    );
    ExitCode::from(2)
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let index = args.iter().position(|arg| arg == name)?;
    args.get(index + 1).map(|s| s.as_str())
}

fn flags(args: &[String], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if arg == name
            && let Some(value) = args.get(index + 1)
        {
            out.push(value.clone());
        }
    }
    out
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn repo_path(args: &[String]) -> PathBuf {
    flag(args, "--repo")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 取位置参数（跳过开关及其值），用于 admit 这类"id + 文件"的命令。
fn positionals(args: &[String], flags_with_value: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        if flags_with_value.contains(&arg.as_str()) {
            index += 2;
            continue;
        }
        if arg.starts_with("--") {
            index += 1;
            continue;
        }
        out.push(arg.clone());
        index += 1;
    }
    out
}

/// 见证对象：优先暂存区（尚未提交的树），否则 HEAD 的树。
fn current_tree(repo: &Repo) -> Result<(trit_vcs::Hash, Option<trit_vcs::Hash>), String> {
    let base = repo.head().map_err(|e| e.to_string())?;
    let base_tree = match base {
        Some(id) => Some(repo.commit_at(&id).map_err(|e| e.to_string())?.tree),
        None => None,
    };
    let tree = match repo.index().map_err(|e| e.to_string())? {
        Some(index) => trit_vcs::Hash::from_hex(&index.tree)
            .ok_or_else(|| format!("index 里的树 id 非法：{}", index.tree))?,
        None => base_tree.ok_or("仓库里既没有暂存区也没有提交；先 add 或 commit")?,
    };
    Ok((tree, base_tree))
}

/// 收集要暂存的文件：路径相对仓库根，目录会递归展开（跳过 `.tritvcs` 与 `.git`）。
fn collect(root: &Path, targets: &[String]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut out = BTreeMap::new();
    let mut push = |path: &Path| -> Result<(), String> {
        let rel = path
            .strip_prefix(root)
            .map_err(|_| format!("不在仓库内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        out.insert(rel, bytes);
        Ok(())
    };
    for target in targets {
        let path = root.join(target);
        if path.is_dir() {
            for entry in walk(&path).map_err(|e| e.to_string())? {
                push(&entry)?;
            }
        } else if path.is_file() {
            push(&path)?;
        } else {
            return Err(format!("找不到：{}", path.display()));
        }
    }
    Ok(out)
}

fn walk(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(current)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == ".tritvcs" || name == ".git" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().cloned() else {
        return usage();
    };
    match command.as_str() {
        "init" => {
            let root = repo_path(&args);
            match Repo::init(&root) {
                Ok(repo) => {
                    println!("已初始化：{}", repo.root().join(".tritvcs").display());
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "add" => {
            let root = repo_path(&args);
            // 目标 = 位置参数；跳过 `--repo` 及其值
            let repo_value = flag(&args, "--repo").map(|v| v.to_string());
            let mut targets: Vec<String> = Vec::new();
            let mut index = 1;
            while index < args.len() {
                let arg = &args[index];
                if arg == "--repo" {
                    index += 2;
                    continue;
                }
                if arg.starts_with("--") || Some(arg.clone()) == repo_value {
                    index += 1;
                    continue;
                }
                targets.push(arg.clone());
                index += 1;
            }
            let mut repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            if targets.is_empty() {
                eprintln!("add 需要至少一个路径");
                return ExitCode::from(2);
            }
            let files = match collect(&root, &targets) {
                Ok(files) => files,
                Err(err) => return fail(err),
            };
            match repo.stage(&files, now_secs()) {
                Ok((tree, put)) => {
                    println!(
                        "已暂存 {} 个文件，树 {}（新块 {} / 复用 {}，写入 {} 字节）",
                        files.len(),
                        tree.short(),
                        put.new_chunks,
                        put.reused_chunks,
                        put.bytes_written
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "commit" => {
            let root = repo_path(&args);
            let message = flag(&args, "-m")
                .or_else(|| flag(&args, "--message"))
                .unwrap_or("");
            let owner = flag(&args, "--owner").unwrap_or("");
            if message.is_empty() || owner.is_empty() {
                eprintln!("commit 需要 -m <说明> 与 --owner <宿主:会话id>");
                return ExitCode::from(2);
            }
            let claims: Vec<ClaimSpec> = match flag(&args, "--claims") {
                Some(path) => match load_claims(Path::new(path)) {
                    Ok(claims) => claims,
                    Err(err) => return fail(err),
                },
                None => Vec::new(),
            };
            let witness = match flag(&args, "--witness") {
                Some(path) => match std::fs::read(path) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => return fail(format!("{path}: {err}")),
                },
                None => None,
            };
            let witness_digest = witness.as_ref().map(|bytes| trit_vcs::Hash::of(bytes));
            let accept = flags(&args, "--accept");
            let intent = match flag(&args, "--goal") {
                Some(goal) => Some(Intent::new(goal, accept)),
                None if !accept.is_empty() => {
                    return fail("--accept 需要同时给 --goal");
                }
                None => None,
            };
            let mut repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let request = CommitRequest {
                message: message.to_string(),
                session: owner.to_string(),
                claims,
                evidence: flags(&args, "--evidence"),
                tags: flags(&args, "--tag"),
                allow_unclaimed: has(&args, "--allow-unclaimed"),
                witness,
                intent,
                parent_override: None,
                now_secs: now_secs(),
            };
            match repo.commit(request) {
                Ok(outcome) => {
                    println!(
                        "提交 {}（门禁 {}{}）",
                        outcome.id.short(),
                        match outcome.verdict {
                            Verdict::Allowed => "allowed".to_string(),
                            Verdict::Unclaimed => "unclaimed（已显式放行）".to_string(),
                            Verdict::Conflict { others } => format!("conflict {others:?}"),
                        },
                        match witness_digest {
                            Some(digest) => format!("，见证 {}", digest.short()),
                            None => String::new(),
                        }
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "witness" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let Some(sep) = args.iter().position(|arg| arg == "--") else {
                eprintln!("witness 需要 `-- <命令>`");
                return ExitCode::from(2);
            };
            let command: Vec<String> = args[sep + 1..].to_vec();
            if command.is_empty() {
                eprintln!("witness 后面没有命令");
                return ExitCode::from(2);
            }
            let (tree, base) = match current_tree(&repo) {
                Ok(pair) => pair,
                Err(err) => return fail(err),
            };
            let started_ms = now_secs() * 1000;
            let start = std::time::Instant::now();
            let output = match std::process::Command::new(&command[0])
                .args(&command[1..])
                .current_dir(repo.root())
                .output()
            {
                Ok(output) => output,
                Err(err) => return fail(format!("启动 {} 失败：{err}", command[0])),
            };
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let witness = Witness::from_run(
                tree,
                base,
                command.clone(),
                RunOutput::new(
                    output.status.code().unwrap_or(-1),
                    output.stdout,
                    output.stderr,
                    started_ms,
                    elapsed_ms,
                ),
            );
            let bytes = witness.encode();
            let path = flag(&args, "--out").map(PathBuf::from).unwrap_or_else(|| {
                repo.root()
                    .join(".tritvcs")
                    .join("witness")
                    .join(format!("{}.json", tree.to_hex()))
            });
            if let Some(parent) = path.parent()
                && let Err(err) = std::fs::create_dir_all(parent)
            {
                return fail(format!("{}: {err}", parent.display()));
            }
            if let Err(err) = std::fs::write(&path, &bytes) {
                return fail(format!("{}: {err}", path.display()));
            }
            println!(
                "见证 {}：{} → 退出 {}，{} ms",
                tree.short(),
                command.join(" "),
                witness.predicate.exit,
                witness.predicate.elapsed_ms
            );
            println!("已写入 {}", path.display());
            if witness.predicate.exit == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        "admit" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let ids = positionals(&args, &["--repo", "--witness"]);
            let Some(target) = ids.first() else {
                eprintln!("admit 需要提交 id");
                return ExitCode::from(2);
            };
            let Some(id) = trit_vcs::Hash::from_hex(target) else {
                return fail(format!("不是合法提交 id：{target}"));
            };
            let Some(path) = flag(&args, "--witness") else {
                eprintln!("admit 需要 --witness FILE");
                return ExitCode::from(2);
            };
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(err) => return fail(format!("{path}: {err}")),
            };
            match repo.admit(&id, &bytes) {
                Ok(witness) => {
                    println!(
                        "准入通过：提交 {}，树 {}，命令 {}，退出 {}",
                        id.short(),
                        witness
                            .subject_tree()
                            .map(|t| t.short())
                            .unwrap_or_default(),
                        witness.predicate.command.join(" "),
                        witness.predicate.exit
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "export" => transport::export_cmd(&args),
        "import" => transport::import_cmd(&args),
        "git-export" => bridge::export_cmd(&args),
        "git-import" => bridge::import_cmd(&args),
        "mcp" => mcp_serve::serve_cmd(&args),
        "merge" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let ids = positionals(&args, &["--repo", "--base"]);
            if ids.len() < 2 {
                eprintln!("merge 需要两个提交 id");
                return ExitCode::from(2);
            }
            let (Some(ours), Some(theirs)) = (
                trit_vcs::Hash::from_hex(&ids[0]),
                trit_vcs::Hash::from_hex(&ids[1]),
            ) else {
                return fail("提交 id 不合法");
            };
            let base = match flag(&args, "--base") {
                Some(value) => match trit_vcs::Hash::from_hex(value) {
                    Some(id) => Some(id),
                    None => return fail(format!("不是合法提交 id：{value}")),
                },
                None => None,
            };
            match merge::merge(&repo, &ours, &theirs, base.as_ref()) {
                Ok(outcome) => {
                    if has(&args, "--json") {
                        let conflicts: Vec<serde_json::Value> = outcome
                            .conflicts
                            .iter()
                            .map(|conflict| {
                                let mut value = serde_json::to_value(conflict).unwrap_or_default();
                                value["object_bytes"] = serde_json::json!(conflict.object_bytes());
                                value["id"] = serde_json::json!(conflict.id().to_hex());
                                value
                            })
                            .collect();
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "tree": outcome.tree.map(|tree| tree.to_hex()),
                                "merged_files": outcome.merged_files,
                                "removed_files": outcome.removed_files,
                                "conflicts": conflicts,
                            }))
                            .unwrap_or_default()
                        );
                    } else if outcome.conflicts.is_empty() {
                        println!(
                            "合并干净：树 {}（{} 个文件合并、{} 个删除）",
                            outcome.tree.map(|tree| tree.short()).unwrap_or_default(),
                            outcome.merged_files,
                            outcome.removed_files
                        );
                    } else {
                        println!(
                            "冲突 {} 处（结构化对象，无冲突标记）：",
                            outcome.conflicts.len()
                        );
                        for conflict in &outcome.conflicts {
                            println!(
                                "  {}  {:?}  hunks={}  对象 {} B / 原文 {} B  id {}",
                                conflict.path,
                                conflict.kind,
                                conflict.hunks.len(),
                                conflict.object_bytes(),
                                conflict.raw_bytes,
                                conflict.id().short()
                            );
                        }
                    }
                    if outcome.conflicts.is_empty() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "where" => query::where_cmd(&args),
        "tests" => query::tests_cmd(&args),
        "explain" => query::explain_cmd(&args),
        "log" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let filter = CommitFilter {
                tag: flag(&args, "--tag").map(str::to_string),
                evidence: flag(&args, "--evidence").map(str::to_string),
                session: flag(&args, "--session").map(str::to_string),
                grep: flag(&args, "--grep").map(str::to_string),
            };
            let log = match repo.query(&filter) {
                Ok(log) => log,
                Err(err) => return fail(err.to_string()),
            };
            if has(&args, "--json") {
                let rows: Vec<_> = log
                    .iter()
                    .map(|(id, commit)| {
                        serde_json::json!({
                            "id": id.to_hex(),
                            "tree": commit.tree.to_hex(),
                            "parents": commit.parents.iter().map(|p| p.to_hex()).collect::<Vec<_>>(),
                            "session": commit.session,
                            "claims": commit.claims,
                            "evidence": commit.evidence,
                            "tags": commit.tags,
                            "message": commit.message,
                            "hlc": [commit.hlc.wall_ms, commit.hlc.counter],
                            "witness": commit.witness.map(|w| w.to_hex()),
                            "intent": commit.intent.as_ref().map(|intent| serde_json::json!({
                                "goal": intent.goal,
                                "accept": intent.accept,
                                "base": intent.base.map(|base| base.to_hex()),
                            })),
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).unwrap_or_default()
                );
            } else if log.is_empty() {
                println!("（还没有提交）");
            } else {
                for (id, commit) in &log {
                    println!(
                        "{}  {}  [{}]  {}{}",
                        id.short(),
                        commit.message,
                        commit.session,
                        if commit.evidence.is_empty() {
                            String::new()
                        } else {
                            format!("evidence:{} ", commit.evidence.join(","))
                        },
                        if commit.tags.is_empty() {
                            String::new()
                        } else {
                            format!("tags:{}", commit.tags.join(","))
                        }
                    );
                }
            }
            ExitCode::SUCCESS
        }
        "status" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let prefix = flag(&args, "--prefix").map(str::to_string);
            match worktree::status_scoped(&repo, prefix.as_deref()) {
                Ok((changes, stats)) => {
                    if has(&args, "--json") {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "changes": changes,
                                "hashed": stats.hashed,
                                "cached": stats.cached,
                                "prefix": prefix,
                            }))
                            .unwrap_or_default()
                        );
                        return ExitCode::SUCCESS;
                    }
                    if changes.is_empty() {
                        println!("工作树干净（与 HEAD 逐内容一致）");
                    } else {
                        for change in &changes {
                            println!("  {:?}  {}", change.kind, change.path);
                        }
                    }
                    println!(
                        "（读了 {} 个文件，stat 缓存省了 {} 个）",
                        stats.hashed, stats.cached
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "checkout" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let ids = positionals(&args, &["--repo", "--prefix"]);
            let Some(target) = ids.first() else {
                eprintln!("checkout 需要提交 id");
                return ExitCode::from(2);
            };
            let Some(id) = trit_vcs::Hash::from_hex(target) else {
                return fail(format!("不是合法提交 id：{target}"));
            };
            let prefix = flag(&args, "--prefix");
            match worktree::checkout_scoped(&repo, &id, prefix, has(&args, "--force")) {
                Ok(stats) => {
                    println!(
                        "已还原 {} 个文件、按范围跳过 {} 个（{}，前缀 {:?}）",
                        stats.written,
                        stats.skipped,
                        id.short(),
                        prefix.unwrap_or("全量")
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "diff" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            let ids: Vec<&str> = args
                .iter()
                .skip(1)
                .filter(|arg| !arg.starts_with("--"))
                .map(|arg| arg.as_str())
                .collect();
            if ids.len() < 2 {
                eprintln!("diff 需要两个提交 id");
                return ExitCode::from(2);
            }
            let (Some(left), Some(right)) = (
                trit_vcs::Hash::from_hex(ids[0]),
                trit_vcs::Hash::from_hex(ids[1]),
            ) else {
                return fail("提交 id 不合法");
            };
            match worktree::diff(&repo, &left, &right) {
                Ok(changes) => {
                    if has(&args, "--json") {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&changes).unwrap_or_default()
                        );
                    } else {
                        for change in &changes {
                            println!("  {:?}  {}", change.kind, change.path);
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "verify" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            match repo.verify() {
                Ok(report) => {
                    println!(
                        "绿：事件 {} 条，对象 {} 个，提交 {} 条（带见证 {} 条），HEAD {}",
                        report.events,
                        report.objects,
                        report.commits,
                        report.witnessed,
                        report.head.unwrap_or_else(|| "-".to_string())
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "stats" => {
            let root = repo_path(&args);
            let repo = match Repo::open(&root) {
                Ok(repo) => repo,
                Err(err) => return fail(err.to_string()),
            };
            match repo.store().stats() {
                Ok(stats) => {
                    println!(
                        "对象 {} 个，占用 {} 字节（{:.2} MiB）",
                        stats.objects,
                        stats.bytes_stored,
                        stats.bytes_stored as f64 / (1024.0 * 1024.0)
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err.to_string()),
            }
        }
        "-h" | "--help" | "help" => usage(),
        other => {
            eprintln!("未知命令：{other}");
            usage()
        }
    }
}

fn fail(message: impl AsRef<str>) -> ExitCode {
    eprintln!("FAIL {}", message.as_ref());
    ExitCode::FAILURE
}
