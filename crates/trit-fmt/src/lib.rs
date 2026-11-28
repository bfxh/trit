//! 前沿格式化器：JSON 打印的第一版，目标是**与 prettier 3 逐字节一致**。
//!
//! 范围（如实）：只做 JSON。JS/TS/Markdown 返回 [`FormatError::Unsupported`]——
//! 这是 S 级任务卡（`docs/specs/FRONTIER-FORMATTER-TASK.md`）里的负结果，不是遗漏。
//!
//! 打印规则来自对 prettier 3.10.1 的实测（`--parser json`，`printWidth=80`）：
//! - 空容器 `{}` / `[]`；
//! - 单行对象带内边距 `{ "k": 1 }`，单行数组无内边距 `[1, 2]`；
//! - 单行放不下时：对象每成员一行；数组若全是标量则 fill（尽量填满行），含容器则每元素一行；
//! - 数字与字符串保持原文（prettier 不改写数字字面量和转义序列）。

use std::fmt;

const PRINT_WIDTH: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// 语法不在这一版支持面内（JS/TS/Markdown）。
    Unsupported(&'static str),
    /// 输入不是合法 JSON（含位置）。
    Syntax { offset: usize, message: String },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(kind) => write!(f, "unsupported syntax: {kind}"),
            Self::Syntax { offset, message } => write!(f, "syntax error at {offset}: {message}"),
        }
    }
}

impl std::error::Error for FormatError {}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Null,
    Bool(bool),
    /// 保留原始数字文本：prettier 不改写 `1e10`、`-0.5`、超大整数。
    Number(String),
    /// 保留原始字符串字面量（含引号与转义），避免二次转义。
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

/// 按文件名推断语法并格式化；不认识的扩展名明确拒绝，不猜。
pub fn format_path(name: &str, source: &str) -> Result<String, FormatError> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".json") || lower.ends_with(".jsonc") {
        return format_json(source);
    }
    let kind = if lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs") {
        "javascript"
    } else if lower.ends_with(".ts") || lower.ends_with(".mts") || lower.ends_with(".cts") {
        "typescript"
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
        "markdown"
    } else {
        "unknown"
    };
    Err(FormatError::Unsupported(kind))
}

/// 格式化一份 JSON 文本；输出末尾带一个换行（与 prettier 一致）。
pub fn format_json(source: &str) -> Result<String, FormatError> {
    let value = Parser::new(source).parse_document()?;
    let mut out = String::with_capacity(source.len() + source.len() / 8);
    print_value(&value, 0, &mut out);
    out.push('\n');
    Ok(out)
}

struct Parser<'a> {
    bytes: &'a [u8],
    source: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            bytes: source.as_bytes(),
            source,
            at: 0,
        }
    }

    fn parse_document(mut self) -> Result<Value, FormatError> {
        self.skip_ws();
        let value = self.parse_value()?;
        self.skip_ws();
        if self.at != self.bytes.len() {
            return Err(self.syntax("trailing content"));
        }
        Ok(value)
    }

    fn syntax(&self, message: &str) -> FormatError {
        FormatError::Syntax {
            offset: self.at,
            message: message.to_owned(),
        }
    }

    fn skip_ws(&mut self) {
        while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), FormatError> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.syntax(&format!("expected {}", byte as char)))
        }
    }

    fn parse_value(&mut self) -> Result<Value, FormatError> {
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Value::Bool(true)),
            Some(b'f') => self.parse_literal("false", Value::Bool(false)),
            Some(b'n') => self.parse_literal("null", Value::Null),
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => self.parse_number(),
            Some(_) => Err(self.syntax("unexpected byte")),
            None => Err(self.syntax("unexpected end of input")),
        }
    }

    fn parse_literal(&mut self, text: &str, value: Value) -> Result<Value, FormatError> {
        if self.source[self.at..].starts_with(text) {
            self.at += text.len();
            Ok(value)
        } else {
            Err(self.syntax(&format!("expected {text}")))
        }
    }

    fn parse_number(&mut self) -> Result<Value, FormatError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
            self.at += 1;
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if start == self.at {
            return Err(self.syntax("expected number"));
        }
        Ok(Value::Number(self.source[start..self.at].to_owned()))
    }

    /// 字符串保留原始字面量（含两侧引号），只做合法性校验。
    fn parse_string(&mut self) -> Result<String, FormatError> {
        let start = self.at;
        self.expect(b'"')?;
        loop {
            match self.peek() {
                None => return Err(self.syntax("unterminated string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(self.source[start..self.at].to_owned());
                }
                Some(b'\\') => {
                    self.at += 1;
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.at += 1;
                        }
                        Some(b'u') => {
                            self.at += 1;
                            for _ in 0..4 {
                                match self.peek() {
                                    Some(byte) if byte.is_ascii_hexdigit() => self.at += 1,
                                    _ => return Err(self.syntax("bad \\u escape")),
                                }
                            }
                        }
                        _ => return Err(self.syntax("bad escape")),
                    }
                }
                Some(byte) if byte < 0x20 => return Err(self.syntax("control byte in string")),
                Some(_) => self.at += 1,
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, FormatError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.syntax("expected , or ]")),
            }
        }
    }

    fn parse_object(&mut self) -> Result<Value, FormatError> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Value::Object(fields));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.parse_value()?;
            fields.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(fields));
                }
                _ => return Err(self.syntax("expected , or }")),
            }
        }
    }
}

fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

/// prettier 对"元素里还有数组"的数组强制换行（`[[1, 2], [3, 4]]` 16 列也会展开），
/// 且这个强制会向上传播到含它的对象。实测见 fixtures 的 golden。
fn forces_break(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(|item| matches!(item, Value::Array(_))),
        Value::Object(fields) => fields.iter().any(|(_, item)| forces_break(item)),
        _ => false,
    }
}

/// 单行（flat）渲染；用于宽度判断与 fill。
fn flat(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(true) => "true".to_owned(),
        Value::Bool(false) => "false".to_owned(),
        Value::Number(text) => text.clone(),
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_owned();
            }
            let body: Vec<String> = items.iter().map(flat).collect();
            format!("[{}]", body.join(", "))
        }
        Value::Object(fields) => {
            if fields.is_empty() {
                return "{}".to_owned();
            }
            let body: Vec<String> = fields
                .iter()
                .map(|(key, item)| format!("{key}: {}", flat(item)))
                .collect();
            format!("{{ {} }}", body.join(", "))
        }
    }
}

fn indent(level: usize, out: &mut String) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn print_value(value: &Value, level: usize, out: &mut String) {
    let column = current_column(out);
    let inline = flat(value);
    // 顶层直接打印，不加缩进；嵌套值已由调用方写好了前缀（缩进 + 键名）。
    if !forces_break(value) && column + inline.chars().count() <= PRINT_WIDTH {
        out.push_str(&inline);
        return;
    }
    match value {
        Value::Array(items) => print_array(items, level, out),
        Value::Object(fields) => print_object(fields, level, out),
        _ => out.push_str(&inline),
    }
}

fn current_column(out: &str) -> usize {
    match out.rfind('\n') {
        Some(index) => out[index + 1..].chars().count(),
        None => out.chars().count(),
    }
}

fn print_array(items: &[Value], level: usize, out: &mut String) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    if items.iter().all(is_scalar) {
        // prettier 对全标量数组用 fill：尽量把元素填满一行。
        out.push_str("[\n");
        let base = (level + 1) * 2;
        let mut column = base;
        indent(level + 1, out);
        for (index, item) in items.iter().enumerate() {
            let text = flat(item);
            let need_comma = index + 1 < items.len();
            let piece = text.chars().count() + usize::from(need_comma);
            if index > 0 {
                if column + 1 + piece > PRINT_WIDTH {
                    out.push('\n');
                    indent(level + 1, out);
                    column = base;
                } else {
                    out.push(' ');
                    column += 1;
                }
            }
            out.push_str(&text);
            column += text.chars().count();
            if need_comma {
                out.push(',');
                column += 1;
            }
        }
        out.push('\n');
        indent(level, out);
        out.push(']');
        return;
    }
    out.push_str("[\n");
    for (index, item) in items.iter().enumerate() {
        indent(level + 1, out);
        print_value(item, level + 1, out);
        if index + 1 < items.len() {
            out.push(',');
        }
        out.push('\n');
    }
    indent(level, out);
    out.push(']');
}

fn print_object(fields: &[(String, Value)], level: usize, out: &mut String) {
    if fields.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    for (index, (key, value)) in fields.iter().enumerate() {
        indent(level + 1, out);
        out.push_str(key);
        out.push_str(": ");
        print_value(value, level + 1, out);
        if index + 1 < fields.len() {
            out.push(',');
        }
        out.push('\n');
    }
    indent(level, out);
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_containers_have_no_padding() {
        assert_eq!(format_json("{}").unwrap(), "{}\n");
        assert_eq!(format_json("[]").unwrap(), "[]\n");
    }

    #[test]
    fn inline_object_has_padding() {
        assert_eq!(format_json(r#"{"a":1}"#).unwrap(), "{ \"a\": 1 }\n");
    }

    #[test]
    fn numbers_and_strings_keep_their_source_text() {
        let out = format_json(r#"{"e":1e10,"f":-0.5,"s":"a\"b\\c"}"#).unwrap();
        assert!(out.contains("1e10"));
        assert!(out.contains("-0.5"));
        assert!(out.contains(r#""a\"b\\c""#));
    }

    #[test]
    fn rejects_non_json_syntax_by_extension() {
        assert!(matches!(
            format_path("x.js", "let a = 1;"),
            Err(FormatError::Unsupported("javascript"))
        ));
        assert!(matches!(
            format_path("x.md", "# hi"),
            Err(FormatError::Unsupported("markdown"))
        ));
    }
}
