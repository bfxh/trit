//! 极简 JSON 行写入器：只覆盖台账需要的类型，不引 serde。
//!
//! 台账是 append-only 的 JSONL：每行一个自洽记录，`sha256` 把同一文件的
//! 多行串起来。这里只保证**输出是合法 JSON**，不追求通用性。

use std::fmt::Display;

/// 编码一个 JSON 字符串字面量（含引号）。
pub fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 编码数字。
pub fn number(value: impl Display) -> String {
    value.to_string()
}

/// 编码布尔。
pub fn boolean(value: bool) -> String {
    if value { "true".into() } else { "false".into() }
}

/// 编码字符串数组。
pub fn array<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let body: Vec<String> = items.into_iter().map(string).collect();
    format!("[{}]", body.join(","))
}

/// 拼一行对象；值必须已经编码（用上面的 helper）。
pub fn object(fields: &[(&str, String)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{}:{}", string(key), value))
        .collect();
    format!("{{{}}}", body.join(","))
}

#[cfg(test)]
mod tests {
    use super::{array, object, string};

    #[test]
    fn escapes_controls_and_quotes() {
        assert_eq!(string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(string("x\ny"), "\"x\\ny\"");
        assert_eq!(string("\u{1}"), "\"\\u0001\"");
        assert_eq!(string("中文"), "\"中文\"");
    }

    #[test]
    fn object_keeps_order_and_types() {
        let line = object(&[
            ("kind", string("fact")),
            ("value", string("true")),
            ("tags", array(["a", "b"])),
        ]);
        assert_eq!(line, r#"{"kind":"fact","value":"true","tags":["a","b"]}"#);
    }
}
