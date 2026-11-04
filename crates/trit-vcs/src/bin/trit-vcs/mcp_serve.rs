//! `mcp` 子命令：stdio 上跑 MCP 工具面（换行分隔 JSON-RPC）。

use std::io::{BufRead, Write};
use std::process::ExitCode;

use serde_json::json;
use trit_vcs::mcp::Mcp;
use trit_vcs::repo::Repo;

use super::{fail, repo_path};

pub(crate) fn serve_cmd(args: &[String]) -> ExitCode {
    let root = repo_path(args);
    let repo = match Repo::open(&root) {
        Ok(repo) => repo,
        Err(err) => return fail(err.to_string()),
    };
    let mut server = Mcp::new(repo);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => return fail(format!("读 stdin 失败：{err}")),
        };
        if line.trim().is_empty() {
            continue;
        }
        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                let _ = writeln!(
                    out,
                    "{}",
                    json!({"jsonrpc": "2.0", "id": null,
                           "error": {"code": -32700, "message": err.to_string()}})
                );
                let _ = out.flush();
                continue;
            }
        };
        if let Some(response) = server.dispatch(&request) {
            if writeln!(out, "{response}").is_err() {
                break;
            }
            let _ = out.flush();
        }
    }
    ExitCode::SUCCESS
}
