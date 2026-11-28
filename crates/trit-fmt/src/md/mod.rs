//! Markdown 块级格式化：与 prettier 3.10.1（`--parser markdown`，proseWrap=preserve）对齐。
//!
//! 支持面（如实，未覆盖的构造一律返回 [`FormatError::Unsupported`]，不静默改写）：
//! - ATX 标题（`#`..`######`）、段落、分隔线；
//! - 围栏代码块（```` ``` ```` / `~~~`，语言标记原样保留，正文逐字保留）；
//! - 缩进代码块（4 空格，正文逐字保留）；
//! - 引用块（可嵌套，内部按块解析）；
//! - 有序 / 无序列表（可嵌套；紧凑/松散按空行判定）；
//! - GFM 表格（按显示宽度对齐，CJK 算 2 列）；
//! - HTML 块按原文透传。
//!
//! 不做的事：行内强调符归一（`__x__` → `**x**`）、setext 标题、脚注、
//! 代码块内嵌格式化（```js / ```json 的正文 prettier 会再格式化）。

mod parse;
mod table;
mod print;
mod width;

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    Heading { level: u8, text: String },
    Paragraph { lines: Vec<String> },
    Fence { info: String, body: String },
    IndentedCode { body: String },
    Quote(Vec<Block>),
    List(List),
    Rule,
    Table(Table),
    Raw(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct List {
    pub ordered: bool,
    pub start: u64,
    pub items: Vec<ListItem>,
    pub loose: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListItem {
    /// 源码里这一项的编号（有序列表用；无序列表为 0）。
    pub number: u64,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Table {
    pub align: Vec<Align>,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    None,
    Left,
    Right,
    Center,
}

impl fmt::Display for Align {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::None => "---",
            Self::Left => ":--",
            Self::Right => "--:",
            Self::Center => ":-:",
        };
        f.write_str(text)
    }
}

/// 格式化一份 Markdown 文本。
pub(crate) fn format_markdown(source: &str) -> Result<String, crate::FormatError> {
    let blocks = parse::parse_document(source)?;
    if blocks.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::with_capacity(source.len());
    print::print_blocks(&blocks, 0, &mut out);
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    Ok(out)
}
