//! # yaraparse —— YARA 规则文件解析器（N4 后半，子集 + fail-closed）
//!
//! 把 `.yar` 文本解析成 [`crate::sigscan::Rule`]。**解析不了的规则绝不静默
//! 跳过**——返回 [`RuleOutcome::Unsupported`] 带原因，扫描时按 `Unknown`
//! 裁决（沉默不是清白：评估不了的规则不能报"没命中"）。
//!
//! ## 支持的子集
//!
//! - `rule <name> [: tags] { meta: … strings: … condition: … }`（多规则
//!   文件；无 strings 的纯数值规则合法）；
//! - 字符串：`$id = "text"`（修饰符 `nocase` `wide` `ascii` `fullword`
//!   `xor`/`xor(k)`/`xor(a-b)`）、`$id = { hex 串 }`（通配/跳跃同
//!   [`crate::sigscan::Pattern::hex`]）；正则串与 `base64`/`base64wide`/
//!   `private` 修饰——**可解析不可扫描**：串记为
//!   [`crate::sigscan::Pattern::Unscannable`]（带原因），整条规则扫描时
//!   判 `Unknown`，不再拖累成整条 Unsupported；
//! - 条件表达式：`and/or/not`、括号、`$id`、`$id at N`、`$id in (a..b)`、
//!   `any/all/N of them`、`any/all/N of ($x*)`、`true`/`false`；
//! - **数值比较**：`filesize`、`uint8/16/32(lit)`、`#a`（计数）、`@a[N]`
//!   （偏移）与 `< <= > >= == !=`（KB/MB 后缀字面量同认）；
//! - **for-of 循环**：`for N|any|all of them/($x*) : ( 体 )`，体内 `$`、
//!   `#$`、`@$`/`@$[N]` 绑定当前串；
//! - 不可求值但记名保留（求值恒 `Unknown`，不猜、不折叠成假）：
//!   `entrypoint`、模块引用 `pe.number_of_sections`、未知函数
//!   `math.entropy(…)`；
//! - 注释：`//` 行注释与 `/* */` 块注释（词法器识别引号内的注释字符——
//!   字符串里出现 `//` 是合法内容）。
//!
//! ## 如实不支持（规则 → Unsupported，不猜）
//!
//! 算术表达式（`filesize + 1`）、字符串运算符（`contains` 等）、
//! `import` 的模块语义、hex 串上的 `xor`/`fullword` 修饰。
//!
//! ```
//! use trit_re::yaraparse::parse_rules;
//!
//! let src = r#"
//! rule demo {
//!   strings:
//!     $mz = { 4D 5A ?? 00 }
//!   condition:
//!     $mz at 0
//! }
//! "#;
//! let out = parse_rules(src).unwrap();
//! assert_eq!(out.rules.len(), 1);
//! assert!(matches!(out.rules[0], trit_re::yaraparse::RuleOutcome::Parsed(_)));
//! ```

use crate::sigscan::{Pattern, Rule, Unscannable};

// 条件表达式解析（拆出守 800 行预算）。
mod cond;

use self::cond::{CondParser, parse_size};

/// 单条规则的解析结局：可评估，或不可评估（带原因）。
#[derive(Debug, Clone)]
pub enum RuleOutcome {
    Parsed(Rule),
    /// 不可评估——扫描时按 `Unknown` 裁决，原因进报告。
    Unsupported {
        name: String,
        reason: String,
    },
}

/// 解析一个 YARA 规则文件（可含多规则；`import`/`include`/`private`/
/// `global` 照实计入 notes 不展开语义）。
#[derive(Debug, Clone, Default)]
pub struct ParsedFile {
    pub rules: Vec<RuleOutcome>,
    /// 顶层杂项注记（`import "pe"` 之类——模块语义不支持，如实记录）。
    pub notes: Vec<String>,
}

/// 解析 YARA 规则文件文本。结构级错误（花括号不平衡等）返回 `Err`；
/// 单条规则内的问题走 `Unsupported` 不拖累其他规则。
pub fn parse_rules(text: &str) -> Result<ParsedFile, String> {
    let toks = lex(text)?;
    let mut parser = Parser { toks, pos: 0 };
    parser.parse_file()
}

// ── 词法 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// 标识符/关键字/数字（含 `$a` 引用、`0x10` 十六进制）。
    Word(String),
    /// 双引号字符串（已去引号，转义已还原）。
    Str(String),
    /// 正则串 `/…/`——词法层认出，语义层判 Unsupported。
    Regex(String),
    Sym(char),
}

fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                chars.next();
            }
            '/' => {
                chars.next();
                match chars.peek() {
                    Some('/') => {
                        chars.next();
                        while let Some(&n) = chars.peek() {
                            chars.next();
                            if n == '\n' {
                                break;
                            }
                        }
                    }
                    Some('*') => {
                        chars.next();
                        let mut closed = false;
                        while let Some(&n) = chars.peek() {
                            chars.next();
                            if n == '*' && chars.peek() == Some(&'/') {
                                chars.next();
                                closed = true;
                                break;
                            }
                        }
                        if !closed {
                            return Err("块注释未闭合".into());
                        }
                    }
                    _ => {
                        // 正则串：到未转义的 '/' 为止
                        let mut body = String::new();
                        let mut closed = false;
                        while let Some(&n) = chars.peek() {
                            chars.next();
                            if n == '\\' {
                                if let Some(&e) = chars.peek() {
                                    chars.next();
                                    body.push('\\');
                                    body.push(e);
                                }
                                continue;
                            }
                            if n == '/' {
                                closed = true;
                                break;
                            }
                            body.push(n);
                        }
                        if !closed {
                            return Err("正则串未闭合".into());
                        }
                        out.push(Tok::Regex(body));
                    }
                }
            }
            '"' => {
                chars.next();
                let mut body = String::new();
                let mut closed = false;
                while let Some(&n) = chars.peek() {
                    chars.next();
                    match n {
                        '\\' => {
                            if let Some(&e) = chars.peek() {
                                chars.next();
                                body.push(match e {
                                    'n' => '\n',
                                    't' => '\t',
                                    '\\' => '\\',
                                    '"' => '"',
                                    other => other,
                                });
                            }
                        }
                        '"' => {
                            closed = true;
                            break;
                        }
                        other => body.push(other),
                    }
                }
                if !closed {
                    return Err("字符串未闭合".into());
                }
                out.push(Tok::Str(body));
            }
            '{' | '}' | '(' | ')' | ':' | '=' | '[' | ']' | '*' | '.' | ',' | '<' | '>' | '!'
            | '-' => {
                chars.next();
                out.push(Tok::Sym(c));
            }
            _ => {
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace()
                        || matches!(
                            n,
                            '{' | '}' | '(' | ')' | ':' | '=' | '"' | '.' | '<' | '>' | '!' | '-'
                        )
                    {
                        break;
                    }
                    chars.next();
                    word.push(n);
                }
                if !word.is_empty() {
                    out.push(Tok::Word(word));
                }
            }
        }
    }
    Ok(out)
}

// ── 解析 ────────────────────────────────────────────────────────────────

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat_sym(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Sym(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_file(&mut self) -> Result<ParsedFile, String> {
        let mut out = ParsedFile::default();
        while let Some(tok) = self.peek().cloned() {
            match tok {
                Tok::Word(w) if w == "import" || w == "include" => {
                    self.pos += 1;
                    let what = match self.next() {
                        Some(Tok::Str(s)) => s,
                        _ => return Err(format!("{w} 后缺字符串")),
                    };
                    out.notes
                        .push(format!("{w} {what:?}：模块/包含语义不支持，如实记录"));
                }
                Tok::Word(w) if w == "private" || w == "global" => {
                    self.pos += 1; // 修饰词：与后随 rule 一并解析
                }
                Tok::Word(w) if w == "rule" => {
                    self.pos += 1;
                    let outcome = self.parse_rule()?;
                    out.rules.push(outcome);
                }
                other => {
                    return Err(format!("顶层出现意外 token：{other:?}"));
                }
            }
        }
        Ok(out)
    }

    /// `rule <name> [: tag… ] { … }`
    fn parse_rule(&mut self) -> Result<RuleOutcome, String> {
        let name = match self.next() {
            Some(Tok::Word(w)) if w != "rule" => w,
            other => return Err(format!("rule 后缺名字：{other:?}")),
        };
        // 标签列表：` : tag1 tag2` 直到 '{'
        while self.eat_sym(':') {
            while self.peek().is_some_and(|t| matches!(t, Tok::Word(_))) {
                self.pos += 1;
            }
        }
        if !self.eat_sym('{') {
            return Err(format!("规则 {name} 缺 '{{'"));
        }
        let mut meta_keys = 0usize;
        let mut strings: Vec<(String, Pattern)> = Vec::new();
        let mut unsupported: Option<String> = None;
        let mut condition_toks: Vec<Tok> = Vec::new();

        while let Some(tok) = self.peek().cloned() {
            if tok == Tok::Sym('}') {
                self.pos += 1;
                if let Some(reason) = unsupported {
                    return Ok(RuleOutcome::Unsupported { name, reason });
                }
                return self.assemble_rule(name, strings, &condition_toks, meta_keys);
            }
            match tok {
                Tok::Word(w) if w == "meta" => {
                    self.pos += 1;
                    self.eat_sym(':');
                    // meta 条目：key = value（Str|Word），只计数不展开
                    while !matches!(self.peek(), None | Some(Tok::Sym('}')))
                        && !self.peek_is_section_keyword()
                    {
                        match self.next() {
                            Some(Tok::Word(_)) => {
                                if self.eat_sym('=') {
                                    self.next();
                                    meta_keys += 1;
                                }
                            }
                            _ => break,
                        }
                    }
                }
                Tok::Word(w) if w == "strings" => {
                    self.pos += 1;
                    self.eat_sym(':');
                    while self
                        .peek()
                        .is_some_and(|t| matches!(t, Tok::Word(w) if w.starts_with('$')))
                    {
                        let Some(Tok::Word(id)) = self.next() else {
                            break;
                        };
                        if !self.eat_sym('=') {
                            return Err(format!("规则 {name}：{id} 后缺 '='"));
                        }
                        if let Some(pattern) = self.parse_string_value(&mut unsupported, &name)? {
                            strings.push((id.trim_start_matches('$').to_string(), pattern));
                        }
                    }
                }
                Tok::Word(w) if w == "condition" => {
                    self.pos += 1;
                    self.eat_sym(':');
                    // 收集条件 token 直到规则收尾的 '}'（深度计数防内层括号误判）
                    let mut depth = 0i32;
                    while let Some(t) = self.next() {
                        match &t {
                            Tok::Sym('}') => {
                                if depth == 0 {
                                    // 回退：'}' 留给外层循环
                                    self.pos -= 1;
                                    break;
                                }
                                depth -= 1;
                                condition_toks.push(t);
                            }
                            Tok::Sym('{') | Tok::Sym('(') => {
                                if t == Tok::Sym('{') {
                                    // 条件里不该出现 '{'——按不支持处理并收尾
                                    unsupported = Some("条件里出现 '{'（不支持的结构）".into());
                                }
                                depth += 1;
                                condition_toks.push(t);
                            }
                            Tok::Sym(')') => {
                                depth -= 1;
                                condition_toks.push(t);
                            }
                            other => condition_toks.push(other.clone()),
                        }
                    }
                }
                other => {
                    return Err(format!("规则 {name} 里出现意外 token：{other:?}"));
                }
            }
        }
        Err(format!("规则 {name} 未闭合（缺 '}}'）"))
    }

    fn peek_is_section_keyword(&self) -> bool {
        matches!(
            self.peek(),
            Some(Tok::Word(w)) if w == "strings" || w == "condition"
        )
    }

    /// strings 里的一个值：文本 / hex 块 / 正则。正则与 `base64`/`private`
    /// 修饰**可解析不可扫描**——串记 [`Pattern::Unscannable`]（带原因），
    /// 整条规则扫描时判 Unknown；`nocase`/`wide`/`ascii`/`fullword`/`xor`
    /// 可评估。返回 `Ok(None)` 已不用——所有形态都有归宿。
    fn parse_string_value(
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
                "base64" | "base64wide" => {
                    unscannable = Some(format!("{w} 修饰串不支持（编码展开未实现）"));
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
        if let Value::Regex(body) = &value {
            // 可解析不可扫描：正则引擎不实现——串记原文形态，规则仍解析
            //（其他串照常扫描），整体在扫描时判 Unknown。
            return Ok(Some(Pattern::Unscannable(Unscannable {
                reason: format!("正则串：/{body}/"),
            })));
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

    fn assemble_rule(
        &self,
        name: String,
        strings: Vec<(String, Pattern)>,
        condition_toks: &[Tok],
        meta_keys: usize,
    ) -> Result<RuleOutcome, String> {
        let _ = meta_keys;
        // 无 strings 的纯数值规则合法（filesize/uintN 等）；条件里引用
        // 未声明串的会在 CondParser 里得到明确的 Unsupported 原因。
        let cond = match CondParser::new(condition_toks, &strings).parse_full() {
            Ok(c) => c,
            Err(reason) => {
                return Ok(RuleOutcome::Unsupported { name, reason });
            }
        };
        Ok(RuleOutcome::Parsed(Rule::from_parts(name, strings, cond)))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_rules;

    /// 单元测试已迁至 `tests/yara.rs`（行数预算：本文件 ≤800 行）。
    /// 这里保留一条最小烟测，确保模块在 lib 测试里有存在感。
    #[test]
    fn module_smoke() {
        let src = r#"rule ok { strings: $a = "u" condition: $a }"#;
        let out = parse_rules(src).unwrap();
        assert_eq!(out.rules.len(), 1);
    }
}
