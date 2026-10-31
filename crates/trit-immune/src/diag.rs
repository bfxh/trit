//! 编译器管线桥：`rustc --message-format=json` 诊断流 → 三值层裁决。
//!
//! **语义与扫描器不同**：编译是"检查"不是"扫描"——
//! - 检查（编译、测试）的通过是**权威结论**，可以给 `+1`（同红队拦截）；
//! - 扫描（静态分析）的清白只是"没发现"，**永不 +1**（见 `scan.rs`）。
//! error → 层 `-1`；warning → 层 `0`（可疑，进下一层扫）；干净 → 层 `+1`。

use crate::json;
use crate::trit::Trit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
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
        let severity = match msg.get("level").and_then(|l| l.as_str()) {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            _ => Severity::Note,
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

/// 层裁决（检查类语义）：任一 error → `-1`；否则任一 warning → `0`；干净 → `+1`。
pub fn layer_verdict(diags: &[Diagnostic]) -> Trit {
    if diags.iter().any(|d| d.severity == Severity::Error) {
        Trit::Vulnerable
    } else if diags.iter().any(|d| d.severity == Severity::Warning) {
        Trit::Unknown
    } else {
        Trit::Hardened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308","explanation":null},"spans":[{"file_name":"src/main.rs","line_start":3,"line_end":3,"is_primary":true}]}}"#,
        "\n",
        "\n",
        r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `x`","code":null,"spans":[{"file_name":"src/lib.rs","line_start":7,"line_end":7}]}}"#,
        "\n",
        r#"{"reason":"build-finished","success":false}"#,
        "\n",
        "这不是 JSON 的进度行\n",
    );

    #[test]
    fn parses_rustc_stream() {
        let d = parse_rustc_stream(SAMPLE);
        assert_eq!(d.len(), 2, "build-finished 与垃圾行应被跳过");
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].code.as_deref(), Some("E0308"));
        assert_eq!(d[0].file, "src/main.rs");
        assert_eq!(d[0].line, Some(3));
        assert_eq!(d[1].severity, Severity::Warning);
        assert_eq!(d[1].code, None);
        // 层裁决：检查类语义
        assert_eq!(layer_verdict(&d), Trit::Vulnerable);
        assert_eq!(layer_verdict(&d[1..]), Trit::Unknown);
        assert_eq!(layer_verdict(&[]), Trit::Hardened);
        assert!(has_json(SAMPLE));
        assert!(!has_json("纯文本\n没有 JSON"));
    }
}
