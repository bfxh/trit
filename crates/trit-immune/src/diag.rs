//! 编译器管线桥：`rustc --message-format=json` 诊断流 → 三值层裁决。
//!
//! **语义与扫描器不同**：编译是"检查"不是"扫描"——
//! - 检查（编译、测试）的通过是**权威结论**，可以给 `+1`（同红队拦截）；
//! - 扫描（静态分析）的清白只是"没发现"，**永不 +1**（见 `scan.rs`）；
//!   error → 层 `-1`；warning → 层 `0`（可疑，进下一层扫）；干净 → 层 `+1`。

use crate::json;
use crate::trit::Trit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    /// 级别未识别 / 无法判定：**fail-closed**——不归入良性 `Note`，也不参与 `+1` 记账。
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub file: String,
    pub line: Option<u32>,
    pub message: String,
    pub code: Option<String>,
}

/// 解析 rustc JSON 输出流（每行一个 JSON 对象）。
/// 非编译消息行（如 `build-finished`）与解析失败的行**如实跳过**——
/// 构建工具可能在流里夹带进度文本；调用方可用 `has_json` 区分"空构建"与"垃圾输入"。
pub fn parse_rustc_stream(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = json::parse(line) else {
            continue;
        };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
            continue;
        }
        let Some(msg) = v.get("message") else {
            continue;
        };
        // 级别先去首尾空白、转小写再归并；规范化后仍不在 rustc 规范级别集合内的，
        // 归 [`Severity::Unknown`]（fail-closed），绝不静默当成良性 `Note` 抬成 +1。
        let raw_level = msg.get("level").and_then(|l| l.as_str()).unwrap_or("");
        let severity = match raw_level.trim().to_ascii_lowercase().as_str() {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            "note" | "help" => Severity::Note,
            _ => Severity::Unknown,
        };
        let message = msg
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let code = msg
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());
        let (file, line_no) = msg
            .get("spans")
            .and_then(|s| s.as_arr())
            .and_then(|a| a.first())
            .map(|sp| {
                (
                    sp.get("file_name")
                        .and_then(|f| f.as_str())
                        .unwrap_or("")
                        .to_string(),
                    sp.get("line_start")
                        .and_then(|l| l.as_f64())
                        .map(|f| f as u32),
                )
            })
            .unwrap_or_default();
        out.push(Diagnostic {
            severity,
            file,
            line: line_no,
            message,
            code,
        });
    }
    out
}

/// 流里是否至少有一行合法 JSON（区分"干净构建"与"垃圾输入"）。
pub fn has_json(text: &str) -> bool {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .any(|l| json::parse(l.trim()).is_ok())
}

/// 层裁决（检查类语义）：任一 error → `-1`；否则任一 warning / 未识别级别 → `0`；干净 → `+1`。
pub fn layer_verdict(diags: &[Diagnostic]) -> Trit {
    if diags.iter().any(|d| d.severity == Severity::Error) {
        Trit::Vulnerable
    } else if diags
        .iter()
        .any(|d| d.severity == Severity::Warning || d.severity == Severity::Unknown)
    {
        Trit::Unknown
    } else {
        Trit::Hardened
    }
}

#[cfg(test)]
mod tests;
