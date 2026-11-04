//! MCP 工具面 v0：stdio JSON-RPC 2.0、13 个工具、每个响应带 token 估算，
//! 且都能接 `budget`（读数上限）与 `commit.dry_run`（写前预检）。
//!
//! 目标不是"再包一层 CLI"，而是让智能体**一次调用拿到结构化结论**：状态、提交、冲突对象、准入、
//! 传输、反查都在这里；schema 尽量短（工具描述本身就是上下文成本）。
//! 传输用 MCP stdio 的换行分隔 JSON；没有网络面。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use crate::bundle;
use crate::gate::ClaimSpec;
use crate::hash::Hash;
use crate::merge;
use crate::model::Intent;
use crate::remediation::{self, Context};
use crate::repo::{CommitFilter, CommitRequest, Repo};
use crate::techmap::{Techmap, test_command};
use crate::tokens::{cap_rows, estimate};
use crate::worktree;

pub const PROTOCOL_VERSION: &str = "2024-11-05";

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": props, "required": required},
    })
}

/// 工具清单（13 个；改数量必须同时改测试断言）。
pub fn tools() -> Vec<Value> {
    vec![
        tool(
            "status",
            "工作树 vs HEAD：改动列表 + stat 缓存读数",
            json!({}),
            &[],
        ),
        tool(
            "log",
            "按 tag/evidence/session/grep 过滤提交",
            json!({
                "tag": {"type": "string"}, "evidence": {"type": "string"},
                "session": {"type": "string"}, "grep": {"type": "string"},
                "limit": {"type": "integer"},
            }),
            &[],
        ),
        tool(
            "explain",
            "提交的会话/证据/见证/意图 + techmap 技术与测试命令",
            json!({"commit": {"type": "string"}, "techmap": {"type": "string"}}),
            &["commit"],
        ),
        tool(
            "add",
            "暂存路径（文件或目录）",
            json!({"paths": {"type": "array", "items": {"type": "string"}}}),
            &["paths"],
        ),
        tool(
            "commit",
            "提交暂存区；默认拒绝未认领（allow_unclaimed=true 才放行）；dry_run=true 只判决不写盘，被拒时文本里直接给下一步命令",
            json!({
                "message": {"type": "string"}, "session": {"type": "string"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "evidence": {"type": "array", "items": {"type": "string"}},
                "goal": {"type": "string"},
                "accept": {"type": "array", "items": {"type": "string"}},
                "witness_path": {"type": "string"},
                "allow_unclaimed": {"type": "boolean"},
                "dry_run": {"type": "boolean"},
            }),
            &["message", "session"],
        ),
        tool(
            "diff",
            "两个提交的路径级差异",
            json!({"from": {"type": "string"}, "to": {"type": "string"}}),
            &["from", "to"],
        ),
        tool(
            "merge",
            "三方合并；冲突返回结构化对象（无冲突标记）",
            json!({"ours": {"type": "string"}, "theirs": {"type": "string"}, "base": {"type": "string"}}),
            &["ours", "theirs"],
        ),
        tool(
            "admit",
            "合并准入：见证必须绑该提交树且通过",
            json!({"commit": {"type": "string"}, "witness_path": {"type": "string"}}),
            &["commit", "witness_path"],
        ),
        tool(
            "bundle_export",
            "导出块级包；prefix 只发一个目录，dest 跳过目标已有对象",
            json!({"tree": {"type": "string"}, "out": {"type": "string"},
                   "prefix": {"type": "string"}, "dest": {"type": "string"}}),
            &["tree", "out"],
        ),
        tool(
            "bundle_import",
            "导入块级包（已有块跳过，逐块校验）",
            json!({"bundle": {"type": "string"}}),
            &["bundle"],
        ),
        tool(
            "where",
            "techmap 反查：路径/测试/标签 → 技术条目",
            json!({"needle": {"type": "string"}, "techmap": {"type": "string"}}),
            &["needle", "techmap"],
        ),
        tool(
            "tests",
            "techmap 反查后的可跑测试命令",
            json!({"needle": {"type": "string"}, "techmap": {"type": "string"}}),
            &["needle", "techmap"],
        ),
        tool("verify", "对象库/事件链/见证体检", json!({}), &[]),
    ]
    .into_iter()
    .map(with_budget)
    .collect()
}

/// 给每个工具的入参补一个 `budget`（元）：超上限时从尾部丢整行，并把丢了多少写回读数。
fn with_budget(mut entry: Value) -> Value {
    entry["inputSchema"]["properties"]["budget"] = json!({
        "type": "integer",
        "minimum": 1,
        "description": "读数上限（元）",
    });
    entry
}

fn ok_result(value: Value) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": false,
        "_meta": {"usage": {"bytes": text.len(), "tokens_est": estimate(&text)}},
    })
}

fn err_result(message: impl Into<String>) -> Value {
    let text = message.into();
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": true,
        "_meta": {"usage": {"bytes": text.len(), "tokens_est": estimate(&text)}},
    })
}

fn arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

fn arg_strings(args: &Value, name: &str) -> Vec<String> {
    args.get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn hex(value: &str, what: &str) -> Result<Hash, String> {
    Hash::from_hex(value).ok_or_else(|| format!("{what} 不是合法 id：{value}"))
}

fn collect(root: &Path, targets: &[String]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<std::path::PathBuf> =
        targets.iter().map(|target| root.join(target)).collect();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).map_err(|err| err.to_string())? {
                let entry = entry.map_err(|err| err.to_string())?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".tritvcs" || name == ".git" {
                    continue;
                }
                stack.push(entry.path());
            }
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| format!("不在仓库内：{}", path.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel, std::fs::read(&path).map_err(|err| err.to_string())?);
        } else {
            return Err(format!("找不到：{}", path.display()));
        }
    }
    Ok(out)
}

fn techmap(args: &Value) -> Result<Techmap, String> {
    let path = arg(args, "techmap")
        .map(str::to_string)
        .or_else(|| std::env::var("TRIT_TECHMAP").ok())
        .ok_or_else(|| "需要 techmap 路径".to_string())?;
    Techmap::load(&path).map_err(|err| format!("{path}: {err}"))
}

/// MCP 服务状态：持有一个打开的仓库。
pub struct Mcp {
    repo: Repo,
}

impl Mcp {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// 处理一条 JSON-RPC；通知返回 `None`。
    pub fn dispatch(&mut self, request: &Value) -> Option<Value> {
        let id = request.get("id")?.clone();
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        match method {
            "initialize" => Some(json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "trit-vcs", "version": env!("CARGO_PKG_VERSION")},
                }
            })),
            "ping" => Some(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            "tools/list" => {
                let tools = tools();
                let text = serde_json::to_string(&tools).unwrap_or_default();
                Some(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {
                        "tools": tools,
                        "_meta": {"usage": {"bytes": text.len(), "tokens_est": estimate(&text)}},
                    }
                }))
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let result = match self.call(name, &args) {
                    Ok(mut value) => {
                        if let Some(budget) = args.get("budget").and_then(Value::as_u64) {
                            cap_rows(&mut value, budget);
                        }
                        ok_result(value)
                    }
                    Err(message) => err_result(message),
                };
                Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
            }
            other => Some(json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("未知方法：{other}")}
            })),
        }
    }

    fn call(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "status" => {
                let (changes, stats) =
                    worktree::status_with_cache(&self.repo).map_err(|err| err.to_string())?;
                Ok(json!({
                    "changes": changes,
                    "hashed": stats.hashed,
                    "cached": stats.cached,
                }))
            }
            "log" => {
                let filter = CommitFilter {
                    tag: arg(args, "tag").map(str::to_string),
                    evidence: arg(args, "evidence").map(str::to_string),
                    session: arg(args, "session").map(str::to_string),
                    grep: arg(args, "grep").map(str::to_string),
                };
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .unwrap_or(20);
                let rows: Vec<Value> = self
                    .repo
                    .query(&filter)
                    .map_err(|err| err.to_string())?
                    .into_iter()
                    .take(limit)
                    .map(|(id, commit)| {
                        json!({
                            "id": id.to_hex(), "tree": commit.tree.to_hex(),
                            "session": commit.session, "claims": commit.claims,
                            "evidence": commit.evidence, "tags": commit.tags,
                            "message": commit.message,
                            "witness": commit.witness.map(|w| w.to_hex()),
                            "intent": commit.intent.as_ref().map(|i| json!({
                                "goal": i.goal, "accept": i.accept,
                                "base": i.base.map(|b| b.to_hex()),
                            })),
                        })
                    })
                    .collect();
                Ok(json!({"commits": rows, "count": rows.len()}))
            }
            "explain" => {
                let id = hex(arg(args, "commit").ok_or("缺少 commit")?, "commit")?;
                let commit = self.repo.commit_at(&id).map_err(|err| err.to_string())?;
                let map = techmap(args)?;
                let entries = map.for_tags(&commit.tags);
                let mut tests = std::collections::BTreeSet::new();
                for entry in &entries {
                    for spec in &entry.tests {
                        tests.insert(test_command(spec).unwrap_or_else(|| spec.clone()));
                    }
                }
                Ok(json!({
                    "id": id.to_hex(), "tree": commit.tree.to_hex(),
                    "session": commit.session, "claims": commit.claims,
                    "tags": commit.tags, "evidence": commit.evidence,
                    "message": commit.message,
                    "witness": commit.witness.map(|w| w.to_hex()),
                    "intent": commit.intent.as_ref().map(|i| json!({
                        "goal": i.goal, "accept": i.accept,
                        "base": i.base.map(|b| b.to_hex()),
                    })),
                    "entries": entries,
                    "tests": tests.into_iter().collect::<Vec<_>>(),
                }))
            }
            "add" => {
                let paths = arg_strings(args, "paths");
                if paths.is_empty() {
                    return Err("paths 不能为空".to_string());
                }
                let files = collect(self.repo.root(), &paths)?;
                let (tree, put) = self
                    .repo
                    .stage(&files, now_secs())
                    .map_err(|err| err.to_string())?;
                Ok(json!({
                    "tree": tree.to_hex(), "files": files.len(),
                    "new_chunks": put.new_chunks, "reused_chunks": put.reused_chunks,
                }))
            }
            "commit" => {
                let message = arg(args, "message").ok_or("缺少 message")?;
                let session = arg(args, "session").ok_or("缺少 session")?;
                let intent =
                    arg(args, "goal").map(|goal| Intent::new(goal, arg_strings(args, "accept")));
                let witness = match arg(args, "witness_path") {
                    Some(path) => {
                        Some(std::fs::read(path).map_err(|err| format!("{path}: {err}"))?)
                    }
                    None => None,
                };
                let request = CommitRequest {
                    message: message.to_string(),
                    session: session.to_string(),
                    claims: Vec::<ClaimSpec>::new(),
                    evidence: arg_strings(args, "evidence"),
                    tags: arg_strings(args, "tags"),
                    allow_unclaimed: args
                        .get("allow_unclaimed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    witness,
                    intent,
                    parent_override: None,
                    now_secs: now_secs(),
                };
                // 目录先拷出来：解释错误时只需要字符串，不能占着 self 的不可变借用
                let dir = self.repo.root().to_path_buf();
                let where_ctx = || Context { dir: &dir, session };
                // 判决段与 commit() 同源（preflight::decide），所以 dry_run 说放行就真放行
                if args
                    .get("dry_run")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return match self.repo.preflight(&request) {
                        Ok(facts) => {
                            Ok(json!({"dry_run": true, "verdict": facts.verdict, "facts": facts}))
                        }
                        Err(err) => Err(remediation::explain(&err, where_ctx())),
                    };
                }
                let outcome = self
                    .repo
                    .commit(request)
                    .map_err(|err| remediation::explain(&err, where_ctx()))?;
                Ok(json!({"id": outcome.id.to_hex(), "verdict": outcome.verdict.label()}))
            }
            "diff" => {
                let from = hex(arg(args, "from").ok_or("缺少 from")?, "from")?;
                let to = hex(arg(args, "to").ok_or("缺少 to")?, "to")?;
                let changes =
                    worktree::diff(&self.repo, &from, &to).map_err(|err| err.to_string())?;
                Ok(json!({"changes": changes}))
            }
            "merge" => {
                let ours = hex(arg(args, "ours").ok_or("缺少 ours")?, "ours")?;
                let theirs = hex(arg(args, "theirs").ok_or("缺少 theirs")?, "theirs")?;
                let base = match arg(args, "base") {
                    Some(value) => Some(hex(value, "base")?),
                    None => None,
                };
                let outcome = merge::merge(&self.repo, &ours, &theirs, base.as_ref())
                    .map_err(|err| err.to_string())?;
                let conflicts: Vec<Value> = outcome
                    .conflicts
                    .iter()
                    .map(|conflict| {
                        let mut value = serde_json::to_value(conflict).unwrap_or_default();
                        value["object_bytes"] = json!(conflict.object_bytes());
                        value["id"] = json!(conflict.id().to_hex());
                        value
                    })
                    .collect();
                Ok(json!({
                    "tree": outcome.tree.map(|tree| tree.to_hex()),
                    "merged_files": outcome.merged_files,
                    "removed_files": outcome.removed_files,
                    "conflicts": conflicts,
                }))
            }
            "admit" => {
                let id = hex(arg(args, "commit").ok_or("缺少 commit")?, "commit")?;
                let path = arg(args, "witness_path").ok_or("缺少 witness_path")?;
                let bytes = std::fs::read(path).map_err(|err| format!("{path}: {err}"))?;
                let witness = self
                    .repo
                    .admit(&id, &bytes)
                    .map_err(|err| err.to_string())?;
                let subject_tree = witness.subject_tree().map_err(|err| err.to_string())?;
                Ok(json!({
                    "admitted": true,
                    "tree": subject_tree.to_hex(),
                    "command": witness.predicate.command,
                    "exit": witness.predicate.exit,
                }))
            }
            "bundle_export" => {
                let tree = hex(arg(args, "tree").ok_or("缺少 tree")?, "tree")?;
                let out_path = arg(args, "out").ok_or("缺少 out")?;
                let dest = match arg(args, "dest") {
                    Some(path) => Some(Repo::open(path).map_err(|err| err.to_string())?),
                    None => None,
                };
                let mut file =
                    std::fs::File::create(out_path).map_err(|err| format!("{out_path}: {err}"))?;
                let stats = bundle::export(
                    self.repo.store(),
                    tree,
                    arg(args, "prefix"),
                    dest.as_ref().map(|repo| repo.store()),
                    &mut file,
                )
                .map_err(|err| err.to_string())?;
                Ok(json!({
                    "files": stats.files, "objects": stats.objects,
                    "sent_objects": stats.sent_objects,
                    "total_bytes": stats.total_bytes, "sent_bytes": stats.sent_bytes,
                }))
            }
            "bundle_import" => {
                let path = arg(args, "bundle").ok_or("缺少 bundle")?;
                let mut file = std::fs::File::open(path).map_err(|err| format!("{path}: {err}"))?;
                let stats =
                    bundle::import(self.repo.store(), &mut file).map_err(|err| err.to_string())?;
                Ok(json!({
                    "received": stats.received, "skipped": stats.skipped,
                    "bytes": stats.bytes,
                    "tree": stats.tree.map(|tree| tree.to_hex()),
                }))
            }
            "where" => {
                let needle = arg(args, "needle").ok_or("缺少 needle")?;
                let map = techmap(args)?;
                let hits = map.where_entries(needle);
                Ok(json!({"entries": hits, "count": hits.len()}))
            }
            "tests" => {
                let needle = arg(args, "needle").ok_or("缺少 needle")?;
                let map = techmap(args)?;
                let mut seen = std::collections::BTreeSet::new();
                for entry in map.where_entries(needle) {
                    for spec in &entry.tests {
                        seen.insert(test_command(spec).unwrap_or_else(|| spec.clone()));
                    }
                }
                Ok(json!({"commands": seen.into_iter().collect::<Vec<_>>()}))
            }
            "verify" => {
                let report = self.repo.verify().map_err(|err| err.to_string())?;
                Ok(json!({
                    "events": report.events, "objects": report.objects,
                    "commits": report.commits, "witnessed": report.witnessed,
                    "head": report.head,
                }))
            }
            other => Err(format!("未知工具：{other}")),
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
