//! 判决的出口：预检（`preflight` / `commit --dry-run`）、失败解释（带下一步命令）、`--budget` 上限。
//!
//! 三件事都是同一目标的三面：智能体一次调用就知道"行不行 / 为什么不行 / 下一步跑什么"，
//! 以及"这次读回来的东西值多少上下文"。

use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;
use trit_vcs::remediation::{self, Context};
use trit_vcs::repo::{CommitRequest, Repo, RepoError};
use trit_vcs::tokens;

use super::{flag, has};

/// 读数出口：打一个纯 JSON 对象/数组，并按 `--budget` 压上限。
///
/// 顶层是数组时（`log --json`）没地方写 `truncation` ⇒ 丢了行必须在 stderr 喊出来，
/// 不许静默截——静默截是最坏的一种"看起来成功了"。
pub(crate) fn print_json(value: Value, args: &[String]) -> ExitCode {
    let mut value = value;
    let is_array = value.is_array();
    let total = value.as_array().map(|rows| rows.len()).unwrap_or(0);
    let budget = flag(args, "--budget").and_then(|raw| raw.parse::<u64>().ok());
    if let Some(budget) = budget {
        if budget == 0 {
            eprintln!("FAIL --budget 要 ≥1（0 等于要求什么都别返回，那是另一种歧义）");
            return ExitCode::from(2);
        }
        let dropped = tokens::cap_rows(&mut value, budget);
        if is_array && dropped > 0 {
            eprintln!(
                "note: --budget {budget} 下丢了 {dropped}/{total} 行（尾部先丢）；用 --tag/--grep/--prefix 缩小范围，或加大 --budget"
            );
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
    );
    ExitCode::SUCCESS
}

/// 判决类失败：除了 `FAIL <原因>`，还把可直接执行的下一步编号打出来。
/// `--json` 时 stdout 是一个对象（`kind` 可分支），stderr 仍带 FAIL 前缀。
pub(crate) fn fail_repo(err: &RepoError, dir: &Path, session: &str, args: &[String]) -> ExitCode {
    let ctx = Context {
        dir,
        session: if session.is_empty() {
            "<宿主:会话id>"
        } else {
            session
        },
    };
    if has(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&remediation::explain_json(err, ctx))
                .unwrap_or_else(|_| "{}".to_string())
        );
    }
    let mut line = format!("FAIL {}", err);
    for (index, command) in remediation::commands(err, ctx).iter().enumerate() {
        line.push_str(&format!("\n  下一步 {}. {}", index + 1, command));
    }
    eprintln!("{line}");
    ExitCode::FAILURE
}

/// `preflight` 与 `commit --dry-run` 的公共实现：跑 `Repo::preflight`（与提交同一条判决）。
pub(crate) fn preflight_run(repo: &Repo, request: &CommitRequest, args: &[String]) -> ExitCode {
    let facts = match repo.preflight(request) {
        Ok(facts) => facts,
        Err(err) => return fail_repo(&err, repo.root(), &request.session, args),
    };
    if has(args, "--json") {
        return print_json(
            serde_json::json!({
                "ok": true,
                "dry_run": true,
                "verdict": facts.verdict,
                "facts": facts,
            }),
            args,
        );
    }
    println!(
        "预检：可以提交（判决 {}，树 {}）",
        facts.verdict,
        short(&facts.tree)
    );
    println!(
        "  规模：{} 个文件 / {} 个对象 / {} 字节（暂存条目 {} 条，折叠成 {} 条判决面）",
        facts.size.files,
        facts.size.objects,
        facts.size.bytes,
        facts.staged_paths,
        facts.touched.len()
    );
    println!(
        "  父提交：{}　基线树：{}",
        facts
            .parent
            .as_deref()
            .map(short)
            .unwrap_or_else(|| "（无）".to_string()),
        facts
            .base_tree
            .as_deref()
            .map(short)
            .unwrap_or_else(|| "（无）".to_string())
    );
    match &facts.witness {
        Some(digest) => println!(
            "  见证：{}（命令退出码 {:?}）",
            short(digest),
            facts.witness_exit
        ),
        None if facts.require_witness => println!("  见证：本仓策略要求，但这次没带"),
        None => {}
    }
    if let Some(intent) = &facts.intent {
        println!(
            "  意图：{}（判据 {} 条，基线 {}）",
            intent.goal,
            intent.accept.len(),
            intent
                .base
                .as_deref()
                .map(short)
                .unwrap_or_else(|| "留空".to_string())
        );
    }
    println!("  以上判决与真提交同源：这条命令放行，`commit` 就跑得通。");
    ExitCode::SUCCESS
}

fn short(hex: &str) -> String {
    hex.chars().take(10).collect()
}
