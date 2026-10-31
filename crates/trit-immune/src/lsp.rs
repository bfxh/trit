//! LSP 诊断桥：`textDocument/publishDiagnostics`（rust-analyzer 等语言服务器）
//! → 内核诊断类型 → 三值层裁决。
//!
//! **检查类可以 +1**——语言服务器报干净是权威结论。
//! LSP 行号 0 基，此处统一转为 1 基（与 rustc 对齐）。

use crate::diag::{self, Diagnostic, Severity};
use crate::json::{self, Json};
use crate::journal::Journal;
use crate::trit::Trit;
use crate::NodeId;
use std::collections::BTreeMap;
use std::io;

/// 一条 LSP 诊断（已转换：行号 1 基）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDiagnostic {
    pub uri: String,
    pub line: Option<u32>,
    pub severity: Severity,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

impl LspDiagnostic {
    pub fn into_diag(self) -> Diagnostic {
        Diagnostic {
            severity: self.severity,
            file: self.uri,
            line: self.line,
            message: self.message,
            code: self.code,
        }
    }
}

fn map_severity(v: Option<u32>) -> Severity {
    match v {
        Some(1) => Severity::Error,
        Some(2) => Severity::Warning,
        _ => Severity::Note,
    }
}

/// 解析 LSP 诊断流（每行一个 JSON-RPC 通知或裸 params 对象）。
pub fn parse_lsp_stream(text: &str) -> Vec<LspDiagnostic> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = json::parse(line) else { continue };
        out.extend(parse_notification(&v));
    }
    out
}

fn parse_notification(v: &Json) -> Vec<LspDiagnostic> {
    let params = v.get("params").unwrap_or(v);
    let Some(diags) = params.get("diagnostics").and_then(|d| d.as_arr()) else {
        return Vec::new();
    };
    let uri = params
        .get("uri")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    diags.iter()
        .map(|d| {
            let severity = map_severity(
                d.get("severity").and_then(|s| s.as_f64()).map(|f| f as u32),
            );
            let line = d
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_f64())
                .map(|f| f as u32 + 1);
            let code = d.get("code").map(|c| match c {
                Json::Str(s) => s.clone(),
                Json::Num(n) => format!("{}", *n as i64),
                other => format!("{other:?}"),
            });
            let source = d
                .get("source")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            let message = d
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            LspDiagnostic {
                uri: uri.clone(),
                line,
                severity,
                code,
                source,
                message,
            }
        })
        .collect()
}

/// 层裁决（检查类语义）：任一 error → `-1`；否则任一 warning → `0`；干净 → `+1`。
pub fn layer_verdict(diags: &[LspDiagnostic]) -> Trit {
    let inner: Vec<Diagnostic> = diags.iter().cloned().map(|d| d.into_diag()).collect();
    diag::layer_verdict(&inner)
}

/// **LSP 实时流桥**：持续消费 rust-analyzer 的 publishDiagnostics，
/// 按 URI 自动归属到对应节点，严重级映射为三值裁决，批量入账。
pub struct LspBridge<'a> {
    pub journal: &'a mut Journal,
    pub uri_map: BTreeMap<String, NodeId>,
}

#[derive(Debug, Clone, Default)]
pub struct LspBatchReport {
    pub notifications: usize,
    pub mapped: usize,
    pub unmapped: usize,
    pub verdicts: Vec<(NodeId, Trit)>,
}

impl LspBridge<'_> {
    /// 处理一批 LSP 通知流。
    pub fn process(&mut self, stream_text: &str) -> io::Result<LspBatchReport> {
        let diags = parse_lsp_stream(stream_text);
        let mut report = LspBatchReport {
            notifications: diags.len(),
            ..Default::default()
        };
        let mut by_uri: BTreeMap<String, Vec<&LspDiagnostic>> = BTreeMap::new();
        for d in &diags {
            by_uri.entry(d.uri.clone()).or_default().push(d);
        }
        for (uri, group) in by_uri {
            let Some(&node) = self.uri_map.get(&uri) else {
                report.unmapped += group.len();
                continue;
            };
            report.mapped += group.len();
            let has_error = group.iter().any(|d| d.severity == Severity::Error);
            let has_warning = group.iter().any(|d| d.severity == Severity::Warning);
            let verdict = if has_error {
                Trit::Vulnerable
            } else if has_warning {
                Trit::Unknown
            } else {
                Trit::Hardened
            };
            let ecount = group.iter().filter(|d| d.severity == Severity::Error).count();
            let wcount = group.iter().filter(|d| d.severity == Severity::Warning).count();
            let first = group.first().map(|d| d.message.as_str()).unwrap_or("");
            self.journal
                .record(node, verdict, None, &format!("LSP {uri}: {ecount}E/{wcount}W　{first}"))
                .map_err(|e| io::Error::other(e.to_string()))?;
            report.verdicts.push((node, verdict));
        }
        Ok(report)
    }

    pub fn map_uri(&mut self, uri: &str, node: NodeId) {
        self.uri_map.insert(uri.to_string(), node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;

    const STREAM: &str = concat!(
        r#"{"params":{"uri":"file:///src/main.rs","diagnostics":[{"range":{"start":{"line":2}},"severity":1,"code":"E0308","source":"rustc","message":"mismatched types"},{"range":{"start":{"line":6,"character":0}},"severity":3,"message":"hint 级"}]}}"#,
        "\n",
        r#"{"params":{"uri":"file:///src/lib.rs","diagnostics":[{"range":{"start":{"line":9,"character":0}},"severity":2,"code":308,"source":"rust-analyzer","message":"unused"}]}}"#,
        "\n",
        "纯文本行如实跳过\n",
    );

    #[test]
    fn parses_full_notification_and_bare_params() {
        let d = parse_lsp_stream(STREAM);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].line, Some(3));
        assert_eq!(d[0].code.as_deref(), Some("E0308"));
        assert_eq!(d[2].severity, Severity::Warning);
        assert_eq!(d[2].code.as_deref(), Some("308"));
    }

    #[test]
    fn layer_verdict_uses_check_semantics() {
        let d = parse_lsp_stream(STREAM);
        assert_eq!(layer_verdict(&d), Trit::Vulnerable);
        let warn_only = parse_lsp_stream(
            r#"{"params":{"uri":"file:///x","diagnostics":[{"range":{"start":{"line":0}},"severity":2,"message":"w"}]}}"#,
        );
        assert_eq!(layer_verdict(&warn_only), Trit::Unknown);
        assert_eq!(layer_verdict(&[]), Trit::Hardened);
        let no_sev = parse_lsp_stream(
            r#"{"params":{"uri":"file:///x","diagnostics":[{"range":{"start":{"line":0}},"message":"m"}]}}"#,
        );
        assert_eq!(layer_verdict(&no_sev), Trit::Hardened);
    }

    #[test]
    fn skips_non_json_and_non_diagnostic_lines() {
        assert!(parse_lsp_stream("垃圾\n{\"jsonrpc\":\"2.0\",\"method\":\"exit\"}\n").is_empty());
    }

    #[test]
    fn lsp_bridge_maps_uri_and_records() {
        let pa = std::env::temp_dir().join(format!(
            "trit-immune-lspbridge-{}.journal",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&pa);
        let mut j = Journal::create_as(&pa, "main", 0).unwrap();
        let f1 = j.spawn(NodeId(0), NodeKind::Delta, "main.rs").unwrap();
        j.into_dag().unwrap();

        let mut j = Journal::open_as(&pa, 0).unwrap();
        let mut bridge = LspBridge {
            journal: &mut j,
            uri_map: BTreeMap::new(),
        };
        bridge.map_uri("file:///src/main.rs", f1);

        let stream = concat!(
            r#"{"params":{"uri":"file:///src/main.rs","diagnostics":[{"range":{"start":{"line":2}},"severity":1,"code":"E0308","message":"mismatched"}]}}"#,
            "\n",
            r#"{"params":{"uri":"file:///src/unknown.rs","diagnostics":[{"range":{"start":{"line":0}},"severity":2,"message":"warning here"}]}}"#,
            "\n",
        );
        let report = bridge.process(stream).unwrap();
        assert_eq!(report.notifications, 2);
        assert_eq!(report.mapped, 1, "只有 main.rs 在映射中");
        assert_eq!(report.unmapped, 1);
        assert_eq!(j.dag().node(f1).verdict, Trit::Vulnerable);

        j.into_dag().unwrap();
        let _ = std::fs::remove_file(&pa);
    }
}
