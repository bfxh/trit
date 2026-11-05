//! MCP 工具面 v0：stdio JSON-RPC 2.0，十二个工具，无网络面。
//!
//! 设计目标是让默认模型**一次调用拿到结构化结论**：读工件、验证行为、应用补丁、
//! 求最小补丁、提议单点编辑、审计日志、求差集、合并。每次调用都从 WSP1 文本开始，
//! 不依赖服务端保存状态。

use std::io::{self, BufRead, Write as IoWrite};

use serde_json::{Value, json};

use crate::analyze;
use crate::audit::AuditLog;
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

/// 十二个工具。改数量必须同时改测试断言。
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
            "wsp_apply_candidate",
            "直接把 proposal 候选应用为补丁；可选后置检查与追加审计日志（日志以文本返回，不写盘）",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "candidate": {"type": "object", "description": "wsp_propose 的 candidates[i]"},
                "oracle": {"type": "object"},
                "require_after": {"type": "boolean"},
                "log": {"type": "string", "description": "已有 JSONL 日志文本或只读路径；空串表示新日志"},
            }),
            &["candidate"],
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
            "wsp_propose",
            "实测单点/两编辑/三编辑组合的失配变化并按收益排序；不改工件",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "limit": {"type": "integer", "description": "返回条数，默认 16；0=全部"},
                "depth": {"type": "integer", "description": "1=单点，2=配对，3=三组合；默认 1"},
                "pairs": {"type": "boolean", "description": "等价于 depth=2（兼容旧调用）"},
                "max_pairs": {"type": "integer", "description": "配对预算，默认 20000"},
                "max_triples": {"type": "integer", "description": "三组合预算，默认 20000"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_explain",
            "解释单个样本：哪些单点编辑让它翻转/变对；先单样本，再 top-K 全量复核",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "sample": {"type": "integer", "description": "样本下标"},
                "top": {"type": "integer", "description": "复核条数，默认 16；0=全部"},
            }),
            &["oracle", "sample"],
        ),
        tool(
            "wsp_probe",
            "逐行清零/取反测行为漂移（大网读取）；budget 超了按步长均匀抽样并标注",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "budget": {"type": "integer", "description": "最多探多少行，0=全部"},
                "top": {"type": "integer", "description": "返回条数，默认 16；0=全部"},
                "adaptive": {"type": "boolean", "description": "按激活活跃度自适应选行（默认均匀步长）"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_localize",
            "两级定位：自适应选承重行，再在行内做 trit 级扫描",
            json!({
                "artifact": artifact(), "path": {"type": "string"},
                "oracle": {"type": "object"},
                "rows": {"type": "integer", "description": "选多少行，默认 3"},
                "row_budget": {"type": "integer", "description": "行探针预算，0=全部"},
                "max_trits": {"type": "integer", "description": "单行最多探多少位置，默认 64；0=全部"},
                "top_trits": {"type": "integer", "description": "每行返回条数，默认 8；0=全部"},
            }),
            &["oracle"],
        ),
        tool(
            "wsp_audit",
            "校验 JSONL 编辑日志的哈希链；可选从初始工件重放并检查最终校验和",
            json!({
                "log": {"type": "string", "description": "JSONL 文本或路径"},
                "artifact": artifact(), "path": {"type": "string"},
            }),
            &["log"],
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
                let outcome =
                    ops::apply_patch(&net, patch, oracle.as_ref(), require_before, require_after)
                        .map_err(err)?;
                let work = outcome.net;
                let mut value = json!({
                    "report": outcome.report,
                    "applied": outcome.applied,
                    "rolled_back": outcome.rolled_back,
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
            "wsp_apply_candidate" => {
                let net = load(args)?;
                let candidate = args.get("candidate").ok_or("需要 candidate")?;
                let patch = wire::patch_from_candidate(candidate).map_err(err)?;
                let patch_for_log = patch.clone();
                let oracle = optional_oracle(args)?;
                let require_after = flag(args, "require_after");
                if require_after && oracle.is_none() {
                    return Err("require_after 需要 oracle".into());
                }
                let outcome = ops::apply_patch(&net, patch, oracle.as_ref(), false, require_after)
                    .map_err(err)?;
                let work = outcome.net;
                let mut value = json!({
                    "report": outcome.report,
                    "applied": outcome.applied,
                    "rolled_back": outcome.rolled_back,
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
                if let Some(raw) = args.get("log").and_then(Value::as_str) {
                    let text = read_log(raw)?;
                    let mut log = AuditLog::from_jsonl(&text).map_err(err)?;
                    log.verify_chain().map_err(err)?;
                    log.record_with_oracle(
                        "mcp-apply-candidate",
                        &net,
                        &work,
                        patch_for_log,
                        outcome.applied,
                        outcome.rolled_back,
                        oracle.as_ref(),
                    )
                    .map_err(err)?;
                    value["log"] = json!(log.to_jsonl());
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
            "wsp_propose" => {
                let net = load(args)?;
                let oracle = required_oracle(args)?;
                let limit = opt_usize(args, "limit", 16);
                let depth = args.get("depth").and_then(Value::as_u64).map_or_else(
                    || {
                        if flag(args, "pairs") { 2 } else { 1 }
                    },
                    |value| value as usize,
                );
                match depth {
                    1 => {
                        let analysis = analyze::analyze(&net, &oracle, limit).map_err(err)?;
                        Ok(wire::analysis_to_value(&analysis))
                    }
                    2 => {
                        let max_pairs = opt_usize(args, "max_pairs", 20_000);
                        let analysis =
                            analyze::analyze_pairs(&net, &oracle, limit, max_pairs).map_err(err)?;
                        Ok(wire::pair_analysis_to_value(&analysis))
                    }
                    3 => {
                        let max_triples = opt_usize(args, "max_triples", 20_000);
                        let analysis = analyze::analyze_triples(&net, &oracle, limit, max_triples)
                            .map_err(err)?;
                        Ok(wire::triple_analysis_to_value(&analysis))
                    }
                    other => Err(format!("depth 只支持 1/2/3，得到 {other}")),
                }
            }
            "wsp_explain" => {
                let net = load(args)?;
                let oracle = required_oracle(args)?;
                let sample = args
                    .get("sample")
                    .and_then(Value::as_u64)
                    .ok_or("需要 sample（非负整数）")? as usize;
                let top = opt_usize(args, "top", 16);
                let explanation =
                    crate::explain::explain_sample(&net, &oracle, sample, top).map_err(err)?;
                Ok(wire::sample_explanation_to_value(&explanation))
            }
            "wsp_probe" => {
                let net = load(args)?;
                let oracle = required_oracle(args)?;
                let budget = opt_usize(args, "budget", 0);
                let top = opt_usize(args, "top", 16);
                let report = if flag(args, "adaptive") {
                    crate::probe::probe_rows_adaptive(&net, &oracle, budget, top).map_err(err)?
                } else {
                    crate::probe::probe_rows(&net, &oracle, budget, top).map_err(err)?
                };
                Ok(wire::probe_report_to_value(&report))
            }
            "wsp_localize" => {
                let net = load(args)?;
                let oracle = required_oracle(args)?;
                let rows = opt_usize(args, "rows", 3);
                let row_budget = opt_usize(args, "row_budget", 0);
                let max_trits = opt_usize(args, "max_trits", 64);
                let top_trits = opt_usize(args, "top_trits", 8);
                let report = crate::localize::localize(
                    &net, &oracle, row_budget, rows, max_trits, top_trits,
                )
                .map_err(err)?;
                Ok(wire::localize_report_to_value(&report))
            }
            "wsp_audit" => {
                let raw = args.get("log").and_then(Value::as_str).ok_or("需要 log")?;
                let text = read_log(raw)?;
                let log = match AuditLog::from_jsonl(&text) {
                    Ok(log) => log,
                    Err(error) => {
                        return Ok(json!({"valid": false, "error": error.to_string()}));
                    }
                };
                if let Err(error) = log.verify_chain() {
                    return Ok(json!({"valid": false, "error": error.to_string()}));
                }
                let mut value = json!({
                    "valid": true,
                    "entries": log.len(),
                    "head": format!("{:016x}", log.head()),
                });
                if let Some(net) = load_optional(args)? {
                    match log.replay(&net) {
                        Ok(report) => {
                            value["replay"] = json!({
                                "entries": report.entries,
                                "applied": report.applied,
                                "rolled_back": report.rolled_back,
                                "skipped": report.skipped,
                                "behavior_checked": report.behavior_checked,
                                "final_checksum": format!("{:016x}", report.final_checksum),
                            });
                        }
                        Err(error) => {
                            value["valid"] = json!(false);
                            value["error"] = json!(error.to_string());
                        }
                    }
                }
                Ok(value)
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

fn read_log(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    if raw.trim_start().starts_with('{') || raw.contains('\n') {
        Ok(raw.to_string())
    } else {
        std::fs::read_to_string(raw).map_err(|error| format!("{raw}: {error}"))
    }
}

fn load_optional(args: &Value) -> Result<Option<Net>, String> {
    if args.get("artifact").is_some() || args.get("path").is_some() {
        load(args).map(Some)
    } else {
        Ok(None)
    }
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
