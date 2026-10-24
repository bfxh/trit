//! MCP 配置体检：起每个 server 做一次 stdio 握手，量启动耗时、工具数、schema 体积，
//! 给出"该关谁 / 该换成直连二进制"的建议。
//!
//! 体积口径是**上下文成本**：`tools/list` 的 schema 字节数（≈ tokens = 字节/4），
//! 不是内存。启动耗时是 spawn → `initialize` 响应，含解释器/包管理器的冷启动。

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub bytes: usize,
}

#[derive(Debug, Default)]
pub struct Session {
    pub tools: Vec<ToolInfo>,
    pub schema_bytes: usize,
}

#[derive(Debug)]
pub struct ServerReport {
    pub name: String,
    pub command: String,
    pub startup_ms: u64,
    pub tools: usize,
    pub schema_bytes: usize,
    pub top_tools: Vec<(String, usize)>,
    pub tool_names: Vec<String>,
    pub recommendations: Vec<(&'static str, String)>,
    pub error: Option<String>,
}

pub fn load_config(text: &str) -> Result<Vec<ServerConfig>> {
    let value: Value = serde_json::from_str(text).context("MCP 配置不是合法 JSON")?;
    let servers = value
        .get("mcpServers")
        .or_else(|| value.get("servers"))
        .and_then(Value::as_object)
        .context("配置里找不到 mcpServers / servers 对象")?;
    let mut out = Vec::new();
    for (name, entry) in servers {
        let Some(command) = entry.get("command").and_then(Value::as_str) else {
            continue; // 远程 server（url）不在本工具范围
        };
        let args = entry
            .get("args")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let env = entry
            .get("env")
            .and_then(Value::as_object)
            .map(|pairs| {
                pairs
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(ServerConfig {
            name: name.clone(),
            command: command.to_string(),
            args,
            env,
        });
    }
    if out.is_empty() {
        bail!("配置里没有 stdio 类型的 server");
    }
    Ok(out)
}

fn wait_for(receiver: &mpsc::Receiver<Value>, id: u64, timeout: Duration) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        if left.is_zero() {
            bail!("等 id={id} 的响应超时");
        }
        match receiver.recv_timeout(left) {
            Ok(message) => {
                if let Some(error) = message.get("error") {
                    bail!("server 返回错误：{error}");
                }
                if message.get("id").and_then(Value::as_u64) == Some(id) {
                    return Ok(message);
                }
            }
            Err(_) => bail!("等 id={id} 的响应超时"),
        }
    }
}

/// 通用握手：读端随便给（进程 stdout / 内存 Cursor），写端同理。
pub fn session<R, W>(reader: R, mut writer: W, timeout: Duration) -> Result<Session>
where
    R: Read + Send + 'static,
    W: Write,
{
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && sender.send(value).is_err()
            {
                break;
            }
        }
    });

    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "trit-procscan", "version": env!("CARGO_PKG_VERSION")},
        }
    });
    writeln!(writer, "{initialize}").context("写 initialize 失败")?;
    writer.flush().ok();
    wait_for(&receiver, 1, timeout)?;

    writeln!(
        writer,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .context("写 initialized 通知失败")?;
    writer.flush().ok();

    writeln!(
        writer,
        "{}",
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    )
    .context("写 tools/list 失败")?;
    writer.flush().ok();
    let listing = wait_for(&receiver, 2, timeout)?;

    let tools_value = listing
        .get("result")
        .and_then(|result| result.get("tools"))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let schema_bytes = serde_json::to_string(&tools_value)
        .map(|text| text.len())
        .unwrap_or(0);
    let mut tools: Vec<ToolInfo> = tools_value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| ToolInfo {
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    bytes: serde_json::to_string(item)
                        .map(|text| text.len())
                        .unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    tools.sort_by_key(|tool| std::cmp::Reverse(tool.bytes));
    Ok(Session {
        tools,
        schema_bytes,
    })
}

pub fn profile(config: &ServerConfig, timeout: Duration) -> ServerReport {
    let mut report = ServerReport {
        name: config.name.clone(),
        command: config.command.clone(),
        startup_ms: 0,
        tools: 0,
        schema_bytes: 0,
        top_tools: Vec::new(),
        tool_names: Vec::new(),
        recommendations: Vec::new(),
        error: None,
    };
    let started = Instant::now();
    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in &config.env {
        command.env(key, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            report.error = Some(format!("启动失败：{error}"));
            return report;
        }
    };
    let stdin = child.stdin.take().expect("stdin 已 pipe");
    let stdout = child.stdout.take().expect("stdout 已 pipe");
    match session(stdout, stdin, timeout) {
        Ok(session) => {
            report.startup_ms = started.elapsed().as_millis() as u64;
            report.tools = session.tools.len();
            report.schema_bytes = session.schema_bytes;
            report.top_tools = session
                .tools
                .iter()
                .take(5)
                .map(|tool| (tool.name.clone(), tool.bytes))
                .collect();
            report.tool_names = session.tools.iter().map(|tool| tool.name.clone()).collect();
        }
        Err(error) => {
            report.startup_ms = started.elapsed().as_millis() as u64;
            report.error = Some(format!("{error:#}"));
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    let lower = config.command.to_ascii_lowercase();
    if lower.ends_with("npx") || lower.ends_with("npx.cmd") {
        report.recommendations.push((
            "npx-launch",
            "经 npx 启动：每次冷启动要解析包，慢且依赖网络；换成直接二进制或本地安装".into(),
        ));
    }
    if report.startup_ms > 2000 {
        report.recommendations.push((
            "slow-startup",
            format!("握手 {} ms（>2s）：会拖慢每个会话的首轮", report.startup_ms),
        ));
    }
    if report.schema_bytes > 20_000 {
        report.recommendations.push((
            "schema-heavy",
            format!(
                "schema {} B ≈ {} tokens；先砍没人用的工具或精简描述",
                report.schema_bytes,
                report.schema_bytes / 4
            ),
        ));
    }
    if report.tools == 0 && report.error.is_none() {
        report
            .recommendations
            .push(("no-tools", "握手成功但没暴露工具：多半可以不装".into()));
    }
    let mut sorted = report.tool_names.clone();
    sorted.sort();
    let unique = {
        let mut copy = sorted.clone();
        copy.dedup();
        copy.len()
    };
    if unique != sorted.len() {
        report.recommendations.push((
            "duplicate-tool-names",
            "同一个 server 暴露了重名工具，客户端会覆盖其中一个".into(),
        ));
    }
    report
}

pub fn report_to_json(reports: &[ServerReport], cross_duplicates: &[String]) -> Value {
    let total_bytes: usize = reports.iter().map(|report| report.schema_bytes).sum();
    json!({
        "tool": "trit-procscan",
        "mode": "mcp",
        "servers": reports.iter().map(|report| json!({
            "name": report.name,
            "command": report.command,
            "startup_ms": report.startup_ms,
            "tools": report.tools,
            "schema_bytes": report.schema_bytes,
            "schema_tokens_estimate": report.schema_bytes / 4,
            "top_tools": report.top_tools.iter().map(|(name, bytes)| json!({"name": name, "bytes": bytes})).collect::<Vec<_>>(),
            "recommendations": report.recommendations.iter().map(|(id, detail)| json!({"id": id, "detail": detail})).collect::<Vec<_>>(),
            "error": report.error,
        })).collect::<Vec<_>>(),
        "total_schema_bytes": total_bytes,
        "total_schema_tokens_estimate": total_bytes / 4,
        "cross_server_duplicate_tools": cross_duplicates,
        "note": "schema 体积 = tools/list 的 JSON 字节数，≈ tokens = 字节/4；启动耗时含解释器/包管理器冷启动",
    })
}

pub fn has_findings(reports: &[ServerReport], cross_duplicates: &[String]) -> bool {
    !cross_duplicates.is_empty()
        || reports
            .iter()
            .any(|report| !report.recommendations.is_empty() || report.error.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_both_config_shapes() {
        let claude = r#"{"mcpServers":{"a":{"command":"node","args":["s.js"],"env":{"K":"V"}},"remote":{"url":"https://x"}}}"#;
        let configs = load_config(claude).unwrap();
        assert_eq!(configs.len(), 1, "远程 server 应被跳过");
        assert_eq!(configs[0].command, "node");
        assert_eq!(configs[0].args, vec!["s.js"]);
        assert_eq!(configs[0].env.get("K").map(String::as_str), Some("V"));

        let vscode = r#"{"servers":{"b":{"type":"stdio","command":"python","args":["m.py"]}}}"#;
        assert_eq!(load_config(vscode).unwrap().len(), 1);
        assert!(load_config(r#"{"nope":{}}"#).is_err());
    }

    #[test]
    fn handshake_reads_tools_and_measures_schema() {
        let replies = [
            json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{}}}),
            json!({"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}),
            json!({"jsonrpc":"2.0","id":2,"result":{"tools":[
                {"name":"read","description":"read a file","inputSchema":{"type":"object"}},
                {"name":"write","description":"write a file","inputSchema":{"type":"object"}}
            ]}}),
        ]
        .iter()
        .map(|value| format!("{value}\n"))
        .collect::<String>();
        let mut written: Vec<u8> = Vec::new();
        let session_ = session(
            Cursor::new(replies.into_bytes()),
            &mut written,
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(session_.tools.len(), 2);
        assert!(session_.schema_bytes > 50, "schema 字节数应被统计");
        let text = String::from_utf8(written).unwrap();
        assert!(text.contains("\"initialize\""), "{text}");
        assert!(text.contains("notifications/initialized"), "{text}");
        assert!(text.contains("tools/list"), "{text}");
    }

    #[test]
    fn timeout_is_reported_instead_of_hanging() {
        let started = Instant::now();
        let error = session(
            Cursor::new(Vec::new()),
            Vec::new(),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(error.to_string().contains("超时"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
