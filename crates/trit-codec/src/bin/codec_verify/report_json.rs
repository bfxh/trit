//! 复核报告的 JSON 出口（`Report::to_json`）。
//!
//! 读数按原样写：能解析成数字的不加引号，其余按字符串转义。

use super::Report;

impl Report {
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{{\"schema\":1,\"kind\":\"{}\",\"seed\":\"0x{:016x}\",\"quick\":{},\"corpus\":{{\"samples\":{},\"trits\":{}}},\"metrics\":{{",
            self.kind, self.seed, self.quick, self.samples, self.trits
        ));
        for (index, (name, value)) in self.metrics.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&format!("\"{}\":", escape(name)));
            if is_number(value) {
                out.push_str(value);
            } else {
                out.push_str(&format!("\"{}\"", escape(value)));
            }
        }
        out.push_str("},\"checks\":[");
        for (index, check) in self.checks.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"name\":\"{}\",\"cases\":{},\"failed\":{},\"first_failure\":",
                check.name, check.cases, check.failed
            ));
            match &check.first {
                Some(text) => out.push_str(&format!("\"{}\"", escape(text))),
                None => out.push_str("null"),
            }
            out.push('}');
        }
        let (cases, failed) = self.totals();
        out.push_str(&format!(
            "],\"totals\":{{\"cases\":{cases},\"failed\":{failed}}},\"ok\":{}}}\n",
            self.ok()
        ));
        out
    }
}

/// 只把“确实能当 JSON 数字读”的读数去掉引号。
fn is_number(raw: &str) -> bool {
    !raw.is_empty()
        && raw
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
}

/// 只转义 JSON 必需字符；报告里的反例都是 ASCII 字母数字加少量标点。
fn escape(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
