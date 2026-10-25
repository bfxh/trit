//! `trit-lsp`：三进制语言服务器（`.tri` / TTIR-0），stdio + JSON-RPC。
//!
//! 编辑器把 `.tri` 关联到本二进制即可；语义由 `trit-core` / `trit-eval` 提供，
//! 本进程只做协议与呈现（诊断、悬停真值表、补全、文档符号）。

use std::io::{self, BufReader};
use std::process::ExitCode;

use serde_json::{Value, json};
use trit_lsp::framing::{read_message, write_message};
use trit_lsp::server::Server;

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();
    let mut server = Server::new();

    loop {
        let message = match read_message(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) => return exit_code(&server),
            Err(error) => {
                let _ = write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"code": -32700, "message": error.to_string()},
                    }),
                );
                return ExitCode::FAILURE;
            }
        };

        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match (method, id) {
            (Some(method), Some(id)) => {
                let response = match server.request(&method, &params) {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err(error) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": error.code, "message": error.message},
                    }),
                };
                if write_message(&mut writer, &response).is_err() {
                    return ExitCode::FAILURE;
                }
            }
            (Some(method), None) => {
                let is_exit = method == "exit";
                for notification in server.notification(&method, &params) {
                    if write_message(&mut writer, &notification).is_err() {
                        return ExitCode::FAILURE;
                    }
                }
                if is_exit {
                    return exit_code(&server);
                }
            }
            // 客户端对我方请求的响应（当前没有服务端发起请求，忽略）。
            (None, _) => {}
        }
    }
}

/// LSP 约定：先 `shutdown` 再 `exit` 才算正常退出。
fn exit_code(server: &Server) -> ExitCode {
    if server.shutdown_received() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
