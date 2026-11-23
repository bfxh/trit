//! `Pattern` 文本串构造器（从 `pattern.rs` 拆出守模块预算）。

use super::super::{Pattern, TextPattern, TextVariant};

impl Pattern {
    /// 文本串：`nocase`（ASCII 大小写折叠，两形态都生效）；`wide` 对齐
    /// YARA 语义——`false` = 仅 ASCII 原样，`true` = 仅 UTF-16LE 变体。
    pub fn text(needle: &str, nocase: bool, wide: bool) -> Self {
        Self::text_ext(needle, nocase, wide, false, None, false)
    }

    /// 全参构造（解析器用）：`ascii_too` 与 `wide` 同开 = 双变体都试；
    /// `xor = Some((lo, hi))` 密钥闭区间展开（对变体最终字节逐位 XOR）；
    /// `fullword` 词边界。
    pub fn text_ext(
        needle: &str,
        nocase: bool,
        wide: bool,
        ascii_too: bool,
        xor: Option<(u8, u8)>,
        fullword: bool,
    ) -> Self {
        let ascii = TextVariant {
            bytes: needle.as_bytes().to_vec(),
            wide: false,
        };
        let wided = TextVariant {
            bytes: needle
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect(),
            wide: true,
        };
        let variants = match (wide, ascii_too) {
            (false, _) => vec![ascii],
            (true, false) => vec![wided],
            (true, true) => vec![ascii, wided],
        };
        Pattern::Text(TextPattern {
            variants,
            nocase,
            xor,
            fullword,
        })
    }
}
