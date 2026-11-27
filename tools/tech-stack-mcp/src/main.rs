//! trit-tech-stack-mcp：把 bfxh/trit 三进制的技术栈策略暴露为 MCP 工具。
//!
//! 传输：stdio，一行一个 JSON-RPC 2.0 消息（MCP stdio transport）。
//!
//! 工具：
//! - `validate_path`：判断某个写入路径是否被允许（新增 Python 只准测试代码）。
//! - `policy`：返回完整策略（test-only 扩展名、测试路径、存量豁免说明）。

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

/// 只允许作为测试代码使用的扩展名（小写、不带点）。
const TEST_ONLY_EXTENSIONS: &[&str] = &["py"];

fn is_test_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or_default();
    lower.starts_with("tests/")
        || lower.contains("/tests/")
        || lower.starts_with("test/")
        || lower.contains("/test/")
        || lower.starts_with("spec/")
        || lower.contains("/spec/")
        || name == "conftest.py"
        || name.starts_with("test_")
        || name.ends_with("_test.py")
}

fn is_test_only(path: &str) -> bool {
    let ext = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    TEST_ONLY_EXTENSIONS.iter().any(|item| *item == ext)
}

fn validate(path: &str) -> Value {
    let allowed = !is_test_only(path) || is_test_path(path);
    if allowed {
        json!({
            "path": path,
            "allowed": true,
            "rule": if is_test_only(path) { "python-test-only" } else { "project-stack" },
        })
    } else {
        json!({
            "path": path,
            "allowed": false,
            "rule": "python-test-only",
            "reason": ".py is only allowed under test paths (tests/, test_*.py, *_test.py, conftest.py)",
        })
    }
}

fn policy() -> Value {
    json!({
        "schema": "trit.tech-stack-policy/v1",
        "test_only_extensions": TEST_ONLY_EXTENSIONS,
        "test_paths": ["tests/**", "test/**", "spec/**", "**/test_*.py", "**/*_test.py", "**/conftest.py"],
        "grandfather": "existing files are not scanned; only new or overwritten paths are validated",
        "applies_to": ["main-agent", "subagent", "external-cli-subagent"],
        "exception": "user authorizes in the task brief as explicit_override",
    })
}

fn text_result(value: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": value.to_string() }],
        "isError": false,
    })
}

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // 通知（无 id）不响应。
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        let result: Result<Value, (i64, String)> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "trit-tech-stack-mcp", "version": "0.1.0" },
            })),
            "tools/list" => Ok(json!({
                "tools": [
                    {
                        "name": "validate_path",
                        "description": "Check whether an agent may write this path under the bfxh/trit tech-stack policy (new Python is test-only).",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "path": { "type": "string" } },
                            "required": ["path"],
                        },
                    },
                    {
                        "name": "policy",
                        "description": "Return the tech-stack policy: test-only extensions, allowed test paths, grandfathering.",
                        "inputSchema": { "type": "object", "properties": {} },
                    },
                ],
            })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                match name {
                    "validate_path" => {
                        let path = arguments
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        Ok(text_result(validate(path)))
                    }
                    "policy" => Ok(text_result(policy())),
                    other => Err((-32602, format!("unknown tool: {other}"))),
                }
            }
            other => Err((-32601, format!("unknown method: {other}"))),
        };

        let response = match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            }),
        };
        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_is_test_only() {
        for path in [
            "tests/test_math.py",
            "tests/sub/test_x.py",
            "pkg/conftest.py",
            "pkg/test_api.py",
            "pkg/api_test.py",
            "spec/test_flow.py",
            "tests\\test_win.py",
        ] {
            assert_eq!(validate(path)["allowed"], true, "{path}");
        }
        for path in ["src/evil.py", "pkg/main.py", "app/scripts/run.py", "src\\main.py"] {
            let value = validate(path);
            assert_eq!(value["allowed"], false, "{path}");
            assert_eq!(value["rule"], "python-test-only", "{path}");
        }
        for path in ["src/lib.rs", "docs/readme.md", "src/app.ts", "cmd/main.go"] {
            assert_eq!(validate(path)["allowed"], true, "{path}");
        }
    }

    #[test]
    fn policy_exposes_the_rule() {
        let value = policy();
        assert_eq!(value["schema"], "trit.tech-stack-policy/v1");
        assert_eq!(value["test_only_extensions"][0], "py");
        assert!(value["test_paths"].as_array().is_some_and(|paths| !paths.is_empty()));
        assert!(value["grandfather"].as_str().is_some_and(|text| text.contains("not scanned")));
    }
}
