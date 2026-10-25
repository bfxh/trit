//! LSP 位置与字节偏移的互转。
//!
//! LSP 的 `character` 是 **UTF-16 码元**，不是字节也不是 Unicode 标量；`.tri` 里
//! 允许中文注释，所以不能拿 `char_indices().count()` 顶替。

use serde_json::{Value, json};

/// LSP 位置：行、列都从 0 起。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    /// 行（0 起）。
    pub line: u32,
    /// 列（UTF-16 码元，0 起）。
    pub character: u32,
}

impl Position {
    /// 构造。
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }

    /// 转成 LSP 的 `{line, character}`。
    pub fn to_json(self) -> Value {
        json!({"line": self.line, "character": self.character})
    }

    /// 从 LSP 的 `{line, character}` 解析；字段缺失时按 0 处理。
    pub fn from_json(value: &Value) -> Self {
        Self {
            line: value.get("line").and_then(Value::as_u64).unwrap_or(0) as u32,
            character: value.get("character").and_then(Value::as_u64).unwrap_or(0) as u32,
        }
    }
}

/// 字节区间（半开）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    /// 起点。
    pub start: Position,
    /// 终点（不含）。
    pub end: Position,
}

impl Range {
    /// 转成 LSP 的 `{start, end}`。
    pub fn to_json(&self) -> Value {
        json!({"start": self.start.to_json(), "end": self.end.to_json()})
    }
}

/// 行起点表：把字节偏移换成 LSP 位置，或反过来。
pub struct LineIndex {
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    /// 建索引（一次扫描）。
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            starts,
            len: text.len(),
        }
    }

    /// 行数（至少 1）。
    pub fn line_count(&self) -> u32 {
        self.starts.len() as u32
    }

    /// 某行的起始字节。
    pub fn line_start(&self, line: u32) -> Option<usize> {
        self.starts.get(line as usize).copied()
    }

    /// 字节偏移 → LSP 位置（越界时钳到文末）。
    pub fn position(&self, text: &str, byte: usize) -> Position {
        let byte = byte.min(self.len);
        let line = match self.starts.binary_search(&byte) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };
        let start = self.starts[line];
        let character = text[start..byte]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>() as u32;
        Position {
            line: line as u32,
            character,
        }
    }

    /// LSP 位置 → 字节偏移（越界时钳到文末）。
    pub fn offset(&self, text: &str, pos: Position) -> usize {
        let Some(start) = self.line_start(pos.line) else {
            return text.len();
        };
        let mut units = 0u32;
        for (i, ch) in text[start..].char_indices() {
            if units >= pos.character || ch == '\n' {
                return start + i;
            }
            units += ch.len_utf16() as u32;
        }
        text.len()
    }

    /// 字节区间 → LSP 范围。
    pub fn range(&self, text: &str, start: usize, end: usize) -> Range {
        let (a, b) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        Range {
            start: self.position(text, a),
            end: self.position(text, b),
        }
    }

    /// 整篇文档的范围。
    pub fn whole(&self, text: &str) -> Range {
        self.range(text, 0, text.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_positions() {
        let text = "# demo\n(a & b)\n";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 3);
        let amp = text.find('&').expect("有 &");
        assert_eq!(idx.position(text, amp), Position::new(1, 3));
        assert_eq!(idx.offset(text, Position::new(1, 3)), amp);
        assert_eq!(idx.line_start(2), Some(15));
    }

    #[test]
    fn utf16_columns() {
        // 中文注释：char 与 UTF-16 码元一致（都在 BMP），但字节不同。
        let text = "# 三进制\n(a & b)";
        let idx = LineIndex::new(text);
        let amp = text.find('&').expect("有 &");
        assert_eq!(idx.position(text, amp), Position::new(1, 3));
        assert_eq!(idx.offset(text, Position::new(1, 3)), amp);
        // 越界钳制
        assert_eq!(idx.offset(text, Position::new(9, 0)), text.len());
        assert_eq!(idx.position(text, text.len() + 100), Position::new(1, 7));
    }
}
