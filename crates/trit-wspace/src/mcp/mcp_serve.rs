//! MCP stdio 服务循环（从 `mcp.rs` 拆出，控制单文件行数）。
//!
//! 一行一条 JSON-RPC；单行有 16 MiB 字节顶，超限回 `-32600` 并丢弃到换行。

use std::io::{self, BufRead, Read, Write as IoWrite};

use serde_json::{Value, json};

use super::Mcp;
use super::write_response;

/// 单行帧的字节上限（与 `trit-lsp` 的 16 MiB 帧顶对齐）。
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// stdio 服务循环：一行一条 JSON-RPC，响应用换行分隔。
pub fn serve<R: BufRead, W: IoWrite>(reader: R, mut writer: W) -> io::Result<()> {
    let mut server = Mcp::new();
    let mut reader = reader;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = reader
            .by_ref()
            .take(MAX_LINE_BYTES as u64)
            .read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        if buf.last() != Some(&b'\n') {
            let mut sink = Vec::new();
            let mut capped = reader.by_ref().take(MAX_LINE_BYTES as u64);
            capped.read_until(b'\n', &mut sink)?;
            let response = json!({
                "jsonrpc": "2.0", "id": Value::Null,
                "error": {"code": -32600, "message": format!("帧行超过上限 {MAX_LINE_BYTES} 字节")}
            });
            write_response(&mut writer, &response)?;
            continue;
        }
        let line = String::from_utf8_lossy(&buf);
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                let response = json!({
                    "jsonrpc": "2.0", "id": Value::Null,
                    "error": {"code": -32700, "message": format!("JSON 解析失败：{error}")}
                });
                write_response(&mut writer, &response)?;
                continue;
            }
        };
        if let Some(response) = server.dispatch(&request) {
            write_response(&mut writer, &response)?;
        }
    }
    Ok(())
}
