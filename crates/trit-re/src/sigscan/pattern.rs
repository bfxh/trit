//! `Pattern` hex 串构造器（从 sigscan.rs 拆出守模块预算）；文本串见
//! [`super::text_pattern`]。

use super::{HexByte, HexPattern, Jump, MAX_JUMP, Pattern};

/// hex 串段数上限（#632：旧实现逐段递归，段数超栈容量即栈溢出 abort）；
/// 超限按"不支持该规则"结构化拒绝。
pub const MAX_HEX_CHUNKS: usize = 4096;

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
        let chunks = 1 + rest.len();
        if chunks > MAX_HEX_CHUNKS {
            return Err(format!("hex 段数 {chunks} 超过上限 {MAX_HEX_CHUNKS}"));
        }
        Ok(Pattern::Hex(HexPattern { lead, rest }))
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

#[path = "text_pattern.rs"]
mod text_pattern;
