//! JSON-RPC 分发与文档状态。
//!
//! 只实现 `.tri` 需要的最小方法集：`initialize` / `shutdown` / `didOpen` /
//! `didChange` / `didClose` / `didSave` / `hover` / `completion` / `documentSymbol` /
//! `definition` / `references` / `documentHighlight` / `rename`。
//! 未实现的方法按 JSON-RPC 返回 `-32601`，未知通知静默忽略。

use std::collections::HashMap;

use serde_json::{Value, json};
use trit_core::Algebra;

use crate::analysis;
use crate::text::Position;

/// JSON-RPC 错误。
#[derive(Debug)]
pub struct RpcError {
    /// 错误码。
    pub code: i64,
    /// 错误消息。
    pub message: String,
}

/// 服务器状态：打开的文本文档 + 当前三值语义。
pub struct Server {
    texts: HashMap<String, String>,
    algebra: Algebra,
    shutdown: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// 空状态；默认 Kleene 强三值语义。
    pub fn new() -> Self {
        Self {
            texts: HashMap::new(),
            algebra: Algebra::Kleene,
            shutdown: false,
        }
    }

    /// 是否收到过 `shutdown`（`exit` 的退出码用它决定）。
    pub fn shutdown_received(&self) -> bool {
        self.shutdown
    }

    /// 处理请求；`Err` 由调用方包成 JSON-RPC error。
    pub fn request(&mut self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => {
                self.apply_options(params);
                Ok(json!({
                    "capabilities": {
                        "textDocumentSync": 1,
                        "hoverProvider": true,
                        "completionProvider": {
                            "triggerCharacters": [
                                "&", "|", "^", "!", "~", "¬", "(", "<", "=", "→", "⊕", "∨", "∧", "⇒", "≡"
                            ],
                            "resolveProvider": false,
                        },
                        "documentSymbolProvider": true,
                        "definitionProvider": true,
                        "referencesProvider": true,
                        "documentHighlightProvider": true,
                        "renameProvider": true,
                    },
                    "serverInfo": {
                        "name": "trit-lsp",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }))
            }
            "shutdown" => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            "textDocument/hover" => self.hover(params),
            "textDocument/completion" => self.completion(params),
            "textDocument/documentSymbol" => self.symbols(params),
            "textDocument/definition" => self.definition(params),
            "textDocument/references" => self.references(params),
            "textDocument/documentHighlight" => self.highlight(params),
            "textDocument/rename" => self.rename(params),
            other => Err(RpcError {
                code: -32601,
                message: format!("未实现的方法：{other}"),
            }),
        }
    }

    /// 处理通知；返回要发回客户端的通知（目前只有 `publishDiagnostics`）。
    pub fn notification(&mut self, method: &str, params: &Value) -> Vec<Value> {
        match method {
            "textDocument/didOpen" => {
                let Some(doc) = params.get("textDocument") else {
                    return Vec::new();
                };
                let (Some(uri), Some(text)) = (
                    doc.get("uri").and_then(Value::as_str),
                    doc.get("text").and_then(Value::as_str),
                ) else {
                    return Vec::new();
                };
                self.texts.insert(uri.to_string(), text.to_string());
                vec![self.publish(uri)]
            }
            "textDocument/didChange" => {
                let Some(uri) = uri_of(params) else {
                    return Vec::new();
                };
                // 同步方式是 Full（capabilities 里 textDocumentSync = 1）。
                if let Some(text) = params
                    .get("contentChanges")
                    .and_then(Value::as_array)
                    .and_then(|changes| changes.last())
                    .and_then(|change| change.get("text"))
                    .and_then(Value::as_str)
                {
                    self.texts.insert(uri.to_string(), text.to_string());
                }
                vec![self.publish(uri)]
            }
            "textDocument/didClose" => {
                let Some(uri) = uri_of(params) else {
                    return Vec::new();
                };
                self.texts.remove(uri);
                vec![json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {"uri": uri, "diagnostics": []},
                })]
            }
            "textDocument/didSave" => {
                let Some(uri) = uri_of(params) else {
                    return Vec::new();
                };
                vec![self.publish(uri)]
            }
            _ => Vec::new(),
        }
    }

    fn apply_options(&mut self, params: &Value) {
        if let Some(name) = params
            .get("initializationOptions")
            .and_then(|options| options.get("algebra"))
            .and_then(Value::as_str)
            && let Some(algebra) = Algebra::parse(name)
        {
            self.algebra = algebra;
        }
    }

    fn publish(&self, uri: &str) -> Value {
        let text = self.texts.get(uri).map(String::as_str).unwrap_or("");
        let analysis = analysis::analyze(text);
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": analysis::diagnostics_json(text, &analysis),
            },
        })
    }

    fn hover(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(uri) = uri_of(params) else {
            return Ok(Value::Null);
        };
        let Some(text) = self.texts.get(uri) else {
            return Ok(Value::Null);
        };
        let position = params
            .get("position")
            .map(Position::from_json)
            .unwrap_or(Position::new(0, 0));
        let analysis = analysis::analyze(text);
        Ok(
            match analysis::hover(text, position, self.algebra, &analysis) {
                Some((markdown, range)) => json!({
                    "contents": {"kind": "markdown", "value": markdown},
                    "range": range.to_json(),
                }),
                None => Value::Null,
            },
        )
    }

    fn completion(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(uri) = uri_of(params) else {
            return Ok(json!({"isIncomplete": false, "items": []}));
        };
        let text = self.texts.get(uri).map(String::as_str).unwrap_or("");
        let analysis = analysis::analyze(text);
        Ok(json!({
            "isIncomplete": false,
            "items": analysis::completion(&analysis),
        }))
    }

    fn symbols(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(uri) = uri_of(params) else {
            return Ok(json!([]));
        };
        let text = self.texts.get(uri).map(String::as_str).unwrap_or("");
        let analysis = analysis::analyze(text);
        Ok(Value::Array(analysis::symbols(text, &analysis)))
    }

    fn position_of(params: &Value) -> Position {
        params
            .get("position")
            .map(Position::from_json)
            .unwrap_or(Position::new(0, 0))
    }

    fn definition(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(uri) = uri_of(params) else {
            return Ok(Value::Null);
        };
        let Some(text) = self.texts.get(uri) else {
            return Ok(Value::Null);
        };
        let analysis = analysis::analyze(text);
        let position = Self::position_of(params);
        Ok(match crate::nav::definition(text, position, &analysis) {
            Some(range) => json!({
                "uri": uri,
                "range": range.to_json(),
            }),
            None => Value::Null,
        })
    }

    fn references(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(uri) = uri_of(params) else {
            return Ok(json!([]));
        };
        let Some(text) = self.texts.get(uri) else {
            return Ok(json!([]));
        };
        let analysis = analysis::analyze(text);
        let position = Self::position_of(params);
        let include_declaration = params
            .get("context")
            .and_then(|context| context.get("includeDeclaration"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let locations: Vec<Value> =
            crate::nav::references(text, position, include_declaration, &analysis)
                .into_iter()
                .map(|range| json!({"uri": uri, "range": range.to_json()}))
                .collect();
        Ok(Value::Array(locations))
    }

    fn highlight(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(uri) = uri_of(params) else {
            return Ok(Value::Null);
        };
        let Some(text) = self.texts.get(uri) else {
            return Ok(Value::Null);
        };
        let analysis = analysis::analyze(text);
        let position = Self::position_of(params);
        Ok(Value::Array(crate::nav::highlight(
            text, position, &analysis,
        )))
    }

    fn rename(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(uri) = uri_of(params) else {
            return Ok(Value::Null);
        };
        let Some(text) = self.texts.get(uri) else {
            return Ok(Value::Null);
        };
        let analysis = analysis::analyze(text);
        let position = Self::position_of(params);
        let new_name = params.get("newName").and_then(Value::as_str).unwrap_or("");
        Ok(crate::nav::rename(text, uri, position, new_name, &analysis).unwrap_or(Value::Null))
    }
}

fn uri_of(params: &Value) -> Option<&str> {
    params
        .get("textDocument")
        .and_then(|doc| doc.get("uri"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(server: &mut Server, text: &str) -> Value {
        let notifications = server.notification(
            "textDocument/didOpen",
            &json!({"textDocument": {"uri": "file:///a.tri", "text": text}}),
        );
        assert_eq!(notifications.len(), 1);
        notifications[0].clone()
    }

    #[test]
    fn initialize_reports_capabilities() {
        let mut server = Server::new();
        let result = server
            .request(
                "initialize",
                &json!({"initializationOptions": {"algebra": "b3"}}),
            )
            .expect("初始化");
        assert_eq!(result["capabilities"]["hoverProvider"], true);
        assert_eq!(result["capabilities"]["textDocumentSync"], 1);
        assert_eq!(server.algebra, Algebra::Bochvar);
    }

    #[test]
    fn did_open_publishes_diagnostics() {
        let mut server = Server::new();
        let notification = open(&mut server, "a & )");
        let diagnostics = &notification["params"]["diagnostics"];
        assert_eq!(diagnostics.as_array().expect("数组").len(), 1);
    }

    #[test]
    fn hover_and_completion() {
        let mut server = Server::new();
        open(&mut server, "a & b");
        let hover = server
            .request(
                "textDocument/hover",
                &json!({
                    "textDocument": {"uri": "file:///a.tri"},
                    "position": {"line": 0, "character": 2},
                }),
            )
            .expect("悬停");
        assert!(
            hover["contents"]["value"]
                .as_str()
                .expect("md")
                .contains("与")
        );
        let completion = server
            .request(
                "textDocument/completion",
                &json!({"textDocument": {"uri": "file:///a.tri"}, "position": {"line": 0, "character": 0}}),
            )
            .expect("补全");
        assert!(!completion["items"].as_array().expect("数组").is_empty());
    }

    #[test]
    fn unknown_request_is_method_not_found() {
        let mut server = Server::new();
        let error = server
            .request("textDocument/formatting", &json!({}))
            .expect_err("应报错");
        assert_eq!(error.code, -32601);
    }

    #[test]
    fn did_close_clears_diagnostics() {
        let mut server = Server::new();
        open(&mut server, "a & )");
        let notifications = server.notification(
            "textDocument/didClose",
            &json!({"textDocument": {"uri": "file:///a.tri"}}),
        );
        assert_eq!(notifications[0]["params"]["diagnostics"], json!([]));
    }
}
