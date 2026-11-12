//! `Pattern` 构造器（从 sigscan.rs 拆出守模块预算）：hex 串 / 文本串。

use super::{HexByte, HexPattern, Jump, MAX_JUMP, Pattern, TextPattern, TextVariant};

impl Pattern {
    /// 解析 hex 串：两位十六进制（空格分隔可选）、`??` 通配、
    /// `[n]` / `[n-m]` 跳跃（`[2-4]` 与 `[2 - 4]` 两种写法都认）。语法错误如实报错。
    pub fn hex(spec: &str) -> Result<Self, String> {
        let mut lead: Vec<HexByte> = Vec::new();
        let mut rest: Vec<(Jump, Vec<HexByte>)> = Vec::new();
        let mut current = &mut lead;
        // 方括号两侧统一插空格：`[2-4]` 与 `[ 2 - 4 ]` 同一处理路径
        let padded = spec.replace('[', " [ ").replace(']', " ] ");
        let mut tokens = padded.split_whitespace();
        while let Some(tok) = tokens.next() {
            if tok == "[" {
                // 收集到 "]" 为止，拼成跳跃区间（`2` / `2-4` / `2 - 4`）
                let mut span = String::new();
                let closed = loop {
                    match tokens.next() {
                        Some("]") => break true,
                        Some(t) => {
                            if !span.is_empty() {
                                span.push(' ');
                            }
                            span.push_str(t);
                        }
                        None => break false,
                    }
                };
                if !closed {
                    return Err("跳跃未闭合".into());
                }
                let (lo, hi) = match span.split_once('-') {
                    Some((a, b)) => (parse_num(a.trim())?, parse_num(b.trim())?),
                    None => {
                        let n = parse_num(span.trim())?;
                        (n, n)
                    }
                };
                if hi < lo {
                    return Err(format!("跳跃区间反了：{lo}-{hi}"));
                }
                if hi > MAX_JUMP {
                    return Err(format!("跳跃超过上限 {MAX_JUMP}"));
                }
                rest.push((Jump { min: lo, max: hi }, Vec::new()));
                current = &mut rest.last_mut().expect("刚 push").1;
                continue;
            }
            if tok == "]" {
                return Err("多余的 ']'".into());
            }
            current.push(parse_hex_byte(tok)?);
        }
        if lead.is_empty() {
            return Err("hex 串不能以跳跃开头".into());
        }
        if rest.iter().any(|(_, chunk)| chunk.is_empty()) {
            return Err("跳跃后必须有字节段（以跳跃结尾不合法）".into());
        }
        Ok(Pattern::Hex(HexPattern { lead, rest }))
    }

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

fn parse_num(tok: &str) -> Result<u32, String> {
    tok.parse::<u32>()
        .map_err(|_| format!("跳跃数字不合法：{tok:?}"))
}

fn parse_hex_byte(tok: &str) -> Result<HexByte, String> {
    if tok == "??" {
        return Ok(HexByte::Wild);
    }
    if tok.len() != 2 || !tok.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("hex 字节不合法：{tok:?}"));
    }
    Ok(HexByte::Lit(
        u8::from_str_radix(tok, 16).map_err(|e| e.to_string())?,
    ))
}
