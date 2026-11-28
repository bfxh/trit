//! 前沿格式化器：JSON 与 Markdown，目标是**与 prettier 3 逐字节一致**。
//!
//! 范围（如实）：JSON 全量；Markdown 是块级子集（见 [`md`] 的模块注释）；
//! JS/TS 返回 [`FormatError::Unsupported`]——这是 S 级任务卡
//! （`docs/specs/FRONTIER-FORMATTER-TASK.md`）里的负结果，不是遗漏。
//!
//! JSON 打印规则来自对 prettier 3.10.1 的实测（`--parser json`，`printWidth=80`）：
//! - 空容器 `{}` / `[]`；单行对象带内边距 `{ "k": 1 }`，单行数组无内边距 `[1, 2]`；
//! - **源码里跨行的容器保持跨行**；成员之间若有空行，break 时保留一个空行；
//! - 单行放不下时：对象每成员一行；数组若全是标量则 fill，含容器则每元素一行；
//! - 数组元素里还有数组时强制展开，且向上传播到父对象；
//! - 数字与字符串保持原文（prettier 不改写数字字面量与转义序列）。

mod md;
mod parse;
mod print;

use std::fmt;

const PRINT_WIDTH: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// 语法不在这一版支持面内（JS/TS，以及 Markdown 里明确不做的构造）。
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
pub(crate) enum Value {
    Null,
    Bool(bool),
    /// 保留原始数字文本：prettier 不改写 `1e10`、`-0.5`、超大整数。
    Number(String),
    /// 保留原始字符串字面量（含引号与转义），避免二次转义。
    String(String),
    Array(Seq),
    Object(Obj),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Seq {
    pub items: Vec<Node>,
    /// 源码里这个数组跨行（`[` 到 `]` 之间出现过换行）。
    pub multiline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Obj {
    pub fields: Vec<Field>,
    /// 源码里这个对象跨行。
    pub multiline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub value: Value,
    /// 该元素前有空行（连续两个及以上换行）。
    pub blank_before: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub key: String,
    pub value: Value,
    pub blank_before: bool,
}

/// 按文件名推断语法并格式化；不认识的扩展名明确拒绝，不猜。
pub fn format_path(name: &str, source: &str) -> Result<String, FormatError> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".json") || lower.ends_with(".jsonc") {
        return format_json(source);
    }
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        return md::format_markdown(source);
    }
    let kind = if lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs") {
        "javascript"
    } else if lower.ends_with(".ts") || lower.ends_with(".mts") || lower.ends_with(".cts") {
        "typescript"
    } else {
        "unknown"
    };
    Err(FormatError::Unsupported(kind))
}

/// 格式化一份 JSON 文本；输出末尾带一个换行（与 prettier 一致）。
pub fn format_json(source: &str) -> Result<String, FormatError> {
    let value = parse::parse_document(source)?;
    let mut out = String::with_capacity(source.len() + source.len() / 8);
    print::print_value(&value, 0, &mut out);
    out.push('\n');
    Ok(out)
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
    fn dispatches_markdown_by_extension() {
        assert!(format_path("x.md", "# hi").is_ok());
        assert!(format_path("x.markdown", "# hi").is_ok());
    }
}
