//! MCP 工具面 v0：stdio JSON-RPC 2.0，六个工具，无网络面。
//!
//! 设计目标是让默认模型**一次调用拿到结构化结论**：读工件、验证行为、应用补丁、
//! 求最小补丁、求差集、合并。每次调用都从 WSP1 文本开始，不依赖服务端保存状态。

use std::io::{self, BufRead, Write as IoWrite};

use serde_json::{Value, json};

use crate::codec::{from_wsp, to_wsp};
use crate::edit::Patch;
use crate::merge::{merge_aligned, merge_vote};
use crate::net::Net;
use crate::ops;
use crate::oracle::Oracle;
use crate::repair::RepairOptions;
use crate::wire;

/// MCP 协议版本。
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// 粗估 token：ASCII 4 字符 / 1 token，非 ASCII 1 字符 / 1 token。
#[must_use]
pub fn estimate_tokens(text: &str) -> u64 {
    let mut scaled = 0u64;
    for ch in text.chars() {
        scaled += if ch.is_ascii() { 1 } else { 4 };
    }
    scaled.div_ceil(4)
}

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": props, "required": required},
    })
}

/// 六个工具。改数量必须同时改测试断言。
#[must_use]
pub fn tools() -> Vec<Value> {
    let artifact =
        || json!({"type": "string", "description": "WSP1 文本（以 wsp1 开头）或文件路径"});
    vec![
        tool(
            "wsp_read",
            "读权重工件：arch / 权重数 / 校验和 / 规范化 WSP1",
            json!({"artifact": artifact(), "path": {"type": "string"}}),
            &[],
        ),
        tool(
            "wsp_verify",
            "在验证集上逐样本比对，返回失配见证（输入/期望/实际）",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object", "description": "inputs/outputs 或 domain/vars/table"},
                "limit": {"type": "integer", "description": "最多返回几条见证，默认 32"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_patch",
            "事务式应用补丁；可选前置/后置行为检查，失败原样回滚",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "patch": {"type": "object", "description": "version=1 的 edits 列表"},
                "oracle": {"type": "object"},
                "require_before": {"type": "boolean"},
                "require_after": {"type": "boolean"},
            }),
            &["patch"],
        ),
        tool(
            "wsp_repair",
            "无梯度求精确补丁（小网络完备 BFS，大网络束搜索），返回补丁与新工件",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "max_edits": {"type": "integer"},
                "beam_width": {"type": "integer"},
                "exhaustive_states": {"type": "integer"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_diff",
            "两个同架构工件的逐点差集（补丁 JSON + 改动量）",
            json!({"a": artifact(), "b": artifact()}),
            &["a", "b"],
        ),
        tool(
            "wsp_merge",
            "合并两个同架构工件；aligned=true 时先消隐藏单元置换",
            json!({"a": artifact(), "b": artifact(), "aligned": {"type": "boolean"}}),
            &["a", "b"],
        ),
    ]
}

/// MCP 服务（无状态）。
#[derive(Default)]
pub struct Mcp;

impl Mcp {
    /// 新建。
    #[must_use]
    pub fn new() -> Self {
        Self
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
                    "serverInfo": {"name": "trit-wspace", "version": env!("CARGO_PKG_VERSION")},
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
                        "_meta": {"usage": {"bytes": text.len(), "tokens_est": estimate_tokens(&text)}},
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
                    Ok(value) => ok_result(value),
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

    fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "wsp_read" => {
                let net = load(args)?;
                Ok(json!({"summary": wire::net_summary(&net), "wsp1": to_wsp(&net)}))
            }
            "wsp_verify" => {
                let net = load(args)?;
                let oracle = required_oracle(args)?;
                let limit = opt_usize(args, "limit", 32);
                Ok(json!({
                    "exact": oracle.exact(&net).map_err(err)?,
                    "loss": oracle.loss(&net).map_err(err)?,
                    "accuracy": oracle.accuracy(&net).map_err(err)?,
                    "summary": wire::net_summary(&net),
                    "mismatches": wire::mismatches_value(&net, &oracle, limit).map_err(err)?,
                }))
            }
            "wsp_patch" => {
                let net = load(args)?;
                let patch_value = args.get("patch").ok_or_else(|| "需要 patch".to_string())?;
                let patch = wire::patch_from_value(patch_value).map_err(err)?;
                let oracle = optional_oracle(args)?;
                let require_before = flag(args, "require_before");
                let require_after = flag(args, "require_after");
                if (require_before || require_after) && oracle.is_none() {
                    return Err("require_before/require_after 需要 oracle".into());
                }
                let (report, work) =
                    ops::apply_patch(&net, patch, oracle.as_ref(), require_before, require_after)
                        .map_err(err)?;
                let mut value = json!({
                    "report": report,
                    "summary": wire::net_summary(&work),
                    "artifact": to_wsp(&work),
                });
                if let Some(oracle) = &oracle {
                    value["verify"] = json!({
                        "exact": oracle.exact(&work).map_err(err)?,
                        "loss": oracle.loss(&work).map_err(err)?,
                        "mismatches": wire::mismatches_value(&work, oracle, 8).map_err(err)?,
                    });
                }
                Ok(value)
            }
            "wsp_repair" => {
                let net = load(args)?;
                let oracle = required_oracle(args)?;
                let options = RepairOptions {
                    max_edits: opt_usize(args, "max_edits", 8),
                    beam_width: opt_usize(args, "beam_width", 64),
                    exhaustive_pairs: 20_000,
                    exhaustive_states: opt_u64(args, "exhaustive_states", 200_000),
                };
                match ops::repair_net(&net, &oracle, &options).map_err(err)? {
                    Some((found, work)) => Ok(json!({
                        "found": true,
                        "repair": wire::repair_to_value(&found),
                        "summary": wire::net_summary(&work),
                        "artifact": to_wsp(&work),
                        "verify": {
                            "exact": oracle.exact(&work).map_err(err)?,
                            "loss": oracle.loss(&work).map_err(err)?,
                        },
                    })),
                    None => Ok(json!({
                        "found": false,
                        "before_loss": oracle.loss(&net).map_err(err)?,
                    })),
                }
            }
            "wsp_diff" => {
                let a = load_key(args, "a")?;
                let b = load_key(args, "b")?;
                let patch = Patch::diff(&a, &b).map_err(err)?;
                Ok(json!({
                    "patch": wire::patch_to_value(&patch),
                    "cost": wire::cost_to_value(&patch.cost(&a)),
                }))
            }
            "wsp_merge" => {
                let a = load_key(args, "a")?;
                let b = load_key(args, "b")?;
                let merged = if flag(args, "aligned") {
                    merge_aligned(&a, &b).map_err(err)?
                } else {
                    merge_vote(&a, &b).map_err(err)?
                };
                Ok(json!({
                    "summary": wire::net_summary(&merged),
                    "artifact": to_wsp(&merged),
                }))
            }
            other => Err(format!("未知工具：{other}")),
        }
    }
}

/// stdio 服务循环：一行一条 JSON-RPC，响应用换行分隔。
pub fn serve<R: BufRead, W: IoWrite>(reader: R, mut writer: W) -> io::Result<()> {
    let mut server = Mcp::new();
    for line in reader.lines() {
        let line = line?;
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

fn write_response<W: IoWrite>(writer: &mut W, response: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, response).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn ok_result(value: Value) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": false,
        "_meta": {"usage": {"bytes": text.len(), "tokens_est": estimate_tokens(&text)}},
    })
}

fn err_result(message: impl Into<String>) -> Value {
    let text = message.into();
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": true,
        "_meta": {"usage": {"bytes": text.len(), "tokens_est": estimate_tokens(&text)}},
    })
}

fn load(args: &Value) -> Result<Net, String> {
    if let Some(raw) = args.get("artifact").and_then(Value::as_str) {
        return parse_or_path(raw);
    }
    if let Some(path) = args.get("path").and_then(Value::as_str) {
        return read_path(path);
    }
    Err("需要 artifact（WSP1 文本）或 path".into())
}

fn load_key(args: &Value, key: &str) -> Result<Net, String> {
    let raw = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("需要 {key}"))?;
    parse_or_path(raw)
}

fn parse_or_path(raw: &str) -> Result<Net, String> {
    let text = if raw.trim_start().starts_with("wsp1") || raw.contains('\n') {
        raw.to_string()
    } else {
        std::fs::read_to_string(raw).map_err(|error| format!("{raw}: {error}"))?
    };
    from_wsp(&text).map_err(err)
}

fn read_path(path: &str) -> Result<Net, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    from_wsp(&text).map_err(err)
}

fn required_oracle(args: &Value) -> Result<Oracle, String> {
    let value = args
        .get("oracle")
        .ok_or_else(|| "需要 oracle".to_string())?;
    wire::oracle_from_value(value).map_err(err)
}

fn optional_oracle(args: &Value) -> Result<Option<Oracle>, String> {
    args.get("oracle")
        .map(|value| wire::oracle_from_value(value).map_err(err))
        .transpose()
}

fn flag(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn opt_usize(args: &Value, name: &str, default: usize) -> usize {
    args.get(name)
        .and_then(Value::as_u64)
        .map_or(default, |value| value as usize)
}

fn opt_u64(args: &Value, name: &str, default: u64) -> u64 {
    args.get(name).and_then(Value::as_u64).unwrap_or(default)
}

fn err(error: crate::error::Error) -> String {
    error.to_string()
}
