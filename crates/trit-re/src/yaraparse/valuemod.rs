//! 串值解析与修饰符消费（从 yaraparse.rs 拆出守模块预算：拆文件不抬台账）。
//!
//! 文本 / hex / 正则三形态 + 修饰符矩阵（nocase/wide/ascii/fullword/xor/
//! base64/base64wide/private）——base64 组合语义委托 [crate::sigscan::b64]。

use super::cond::parse_size;
use super::{Parser, Tok};
use crate::sigscan::{Pattern, Unscannable};

impl Parser {
    /// strings 里的一个值：文本 / hex 块 / 正则。正则与 `base64`/`private`
    /// 修饰**可解析不可扫描**——串记 [`Pattern::Unscannable`]（带原因），
    /// 整条规则扫描时判 Unknown；`nocase`/`wide`/`ascii`/`fullword`/`xor`
    /// 可评估。返回 `Ok(None)` 已不用——所有形态都有归宿。
    pub(super) fn parse_string_value(
        &mut self,
        unsupported: &mut Option<String>,
        rule_name: &str,
    ) -> Result<Option<Pattern>, String> {
        enum Value {
            Text(String),
            Hex(Pattern),
            Regex(String),
        }
        let value = match self.next() {
            Some(Tok::Str(s)) => Value::Text(s),
            Some(Tok::Regex(b)) => Value::Regex(b),
            Some(Tok::Sym('{')) => {
                // hex 块：吃到 '}'，token 拼回空格分隔的 hex spec
                let mut spec = String::new();
                loop {
                    match self.next() {
                        Some(Tok::Sym('}')) => break,
                        Some(Tok::Word(w)) => {
                            if !spec.is_empty() {
                                spec.push(' ');
                            }
                            spec.push_str(&w);
                        }
                        Some(Tok::Sym(c)) => {
                            if !spec.is_empty() {
                                spec.push(' ');
                            }
                            spec.push(c);
                        }
                        other => {
                            return Err(format!("规则 {rule_name} 的 hex 串未闭合：{other:?}"));
                        }
                    }
                }
                match Pattern::hex(&spec) {
                    Ok(p) => Value::Hex(p),
                    Err(e) => return Err(format!("规则 {rule_name} hex 串不合法：{e}")),
                }
            }
            other => {
                return Err(format!("规则 {rule_name}：字符串值不合法 {other:?}"));
            }
        };
        // 修饰符
        let mut nocase = false;
        let mut wide = false;
        let mut ascii_too = false;
        let mut fullword = false;
        let mut xor: Option<(u8, u8)> = None;
        let mut b64_std = false;
        let mut b64_wide = false;
        let mut unscannable: Option<String> = None;
        while let Some(Tok::Word(w)) = self.peek() {
            match w.as_str() {
                "nocase" => {
                    nocase = true;
                    self.pos += 1;
                }
                "wide" => {
                    wide = true;
                    self.pos += 1;
                }
                "ascii" => {
                    ascii_too = true;
                    self.pos += 1;
                }
                "fullword" => {
                    fullword = true;
                    self.pos += 1;
                }
                "xor" => {
                    self.pos += 1;
                    xor = Some(if self.eat_sym('(') {
                        let lo = parse_size(&match self.next() {
                            Some(Tok::Word(w)) => w,
                            other => return Err(format!("规则 {rule_name}：xor 缺密钥 {other:?}")),
                        })
                        .ok_or_else(|| format!("规则 {rule_name}：xor 密钥不合法"))?;
                        if lo > 0xFF {
                            return Err(format!("规则 {rule_name}：xor 密钥越界（≤0xFF）"));
                        }
                        let keys = if self.eat_sym('-') {
                            let hi = parse_size(&match self.next() {
                                Some(Tok::Word(w)) => w,
                                other => {
                                    return Err(format!("规则 {rule_name}：xor 缺上界 {other:?}"));
                                }
                            })
                            .ok_or_else(|| format!("规则 {rule_name}：xor 上界不合法"))?;
                            if hi > 0xFF || hi < lo {
                                return Err(format!(
                                    "规则 {rule_name}：xor 区间不合法（{lo}-{hi}）"
                                ));
                            }
                            (lo as u8, hi as u8)
                        } else {
                            (lo as u8, lo as u8)
                        };
                        if !self.eat_sym(')') {
                            return Err(format!("规则 {rule_name}：xor 未闭合"));
                        }
                        keys
                    } else {
                        (0x00, 0xFF) // 裸 xor：全键
                    });
                }
                "base64" => {
                    b64_std = true;
                    self.pos += 1;
                }
                "base64wide" => {
                    b64_wide = true;
                    self.pos += 1;
                }
                "private" => {
                    unscannable = Some("private 串不支持（条件引用语义未实现）".into());
                    self.pos += 1;
                }
                _ => break,
            }
        }
        if let Some(reason) = unscannable {
            return Ok(Some(Pattern::Unscannable(Unscannable { reason })));
        }
        if xor.is_some() && nocase {
            // 官方 YARA 编译器拒绝 xor nocase（"invalid modifier combination"，
            // 差分探针实测）——对齐拒绝，不给超集扩展留静默面。
            return Err(format!("规则 {rule_name}：xor 与 nocase 组合官方不合法"));
        }
        if b64_std || b64_wide {
            // base64 全套语义（组合校验/展开/弃权）在 sigscan::b64——守预算
            let pt = match &value {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            };
            return Pattern::base64_plan(
                rule_name,
                pt,
                matches!(value, Value::Regex(_)),
                nocase,
                wide,
                xor.is_some(),
                fullword,
                b64_std,
                b64_wide,
            );
        }
        if let Value::Regex(body) = &value {
            // 正则串：Pike VM（sigscan::regex）；修饰语义在 Pattern::regex 内守
            return Ok(Some(Pattern::regex(
                body,
                nocase,
                wide,
                xor.is_some(),
                fullword,
            )));
        }
        if let Value::Regex(body) = &value {
            // 正则串：Pike VM（sigscan::regex）；修饰语义在 Pattern::regex 内守
            return Ok(Some(Pattern::regex(
                body,
                nocase,
                wide,
                xor.is_some(),
                fullword,
            )));
        }
        let pattern = match value {
            Value::Text(s) => Pattern::text_ext(&s, nocase, wide, ascii_too, xor, fullword),
            Value::Hex(p) => {
                if xor.is_some() || fullword {
                    let _ = unsupported.insert("hex 串上的 xor/fullword 修饰不支持".into());
                }
                p
            }
            Value::Regex(_) => unreachable!("上面已处理"),
        };
        Ok(Some(pattern))
    }
}
