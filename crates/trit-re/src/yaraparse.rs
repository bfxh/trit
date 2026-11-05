//! # yaraparse —— YARA 规则文件解析器（N4 后半，子集 + fail-closed）
//!
//! 把 `.yar` 文本解析成 [`crate::sigscan::Rule`]。**解析不了的规则绝不静默
//! 跳过**——返回 [`RuleOutcome::Unsupported`] 带原因，扫描时按 `Unknown`
//! 裁决（沉默不是清白：评估不了的规则不能报"没命中"）。
//!
//! ## 支持的子集
//!
//! - `rule <name> [: tags] { meta: … strings: … condition: … }`（多规则文件）；
//! - 字符串：`$id = "text"`（修饰符 `nocase` `wide` `ascii`）、
//!   `$id = { hex 串 }`（通配/跳跃同 [`crate::sigscan::Pattern::hex`]）；
//! - 条件表达式：`and/or/not`、括号、`$id`、`$id at N`、`$id in (a..b)`、
//!   `any/all/N of them`、`any/all/N of ($x*)`；
//! - 注释：`//` 行注释与 `/* */` 块注释（词法器识别引号内的注释字符——
//!   字符串里出现 `//` 是合法内容）。
//!
//! ## 如实不支持（规则 → Unsupported，不猜）
//!
//! 正则串（`/…/`）、`filesize`/`pe.*`/`uint*(…)`/`entrypoint` 等非字符串
//! 条件、`#a`/`@a` 计数引用、`for … of` 循环、`import` 的模块语义。
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

use crate::sigscan::{Condition, Pattern, Rule};

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
            '{' | '}' | '(' | ')' | ':' | '=' | '[' | ']' | '*' | '.' | ',' => {
                chars.next();
                out.push(Tok::Sym(c));
            }
            _ => {
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace()
                        || matches!(n, '{' | '}' | '(' | ')' | ':' | '=' | '"' | '.')
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

    /// strings 里的一个值：文本 / hex 块 / 正则（正则 → 整条规则 Unsupported）。
    /// 修饰符在值之后：先收值再收修饰符，最后一次性构造模式。
    /// 返回 `Ok(None)` = 已注定 Unsupported（上层继续吃 token）。
    fn parse_string_value(
        &mut self,
        unsupported: &mut Option<String>,
        rule_name: &str,
    ) -> Result<Option<Pattern>, String> {
        enum Value {
            Text(String),
            Hex(Pattern),
            Regex,
        }
        let value = match self.next() {
            Some(Tok::Str(s)) => Value::Text(s),
            Some(Tok::Regex(_)) => Value::Regex,
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
                    self.pos += 1; // ascii 是默认形态，吃掉即可
                }
                _ => break,
            }
        }
        if matches!(value, Value::Regex) {
            let _ = unsupported.insert("正则串（/…/）不支持".into());
            return Ok(None);
        }
        let pattern = match value {
            Value::Text(s) => Pattern::text(&s, nocase, wide),
            Value::Hex(p) => p,
            Value::Regex => unreachable!("上面已处理"),
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
        if strings.is_empty() && !condition_toks.is_empty() {
            return Ok(RuleOutcome::Unsupported {
                name,
                reason: "条件引用了未声明的串（strings 为空）".into(),
            });
        }
        let mut cp = CondParser {
            toks: condition_toks,
            pos: 0,
            strings: &strings,
        };
        let cond = match cp.parse_expr() {
            Ok(c) => c,
            Err(reason) => {
                return Ok(RuleOutcome::Unsupported { name, reason });
            }
        };
        if cp.pos != cp.toks.len() {
            return Ok(RuleOutcome::Unsupported {
                name,
                reason: format!("条件尾部有未消化的 token：{:?}", cp.toks.get(cp.pos)),
            });
        }
        Ok(RuleOutcome::Parsed(Rule::from_parts(name, strings, cond)))
    }
}

// ── 条件表达式（递归下降）───────────────────────────────────────────────

struct CondParser<'a> {
    toks: &'a [Tok],
    pos: usize,
    strings: &'a [(String, Pattern)],
}

impl CondParser<'_> {
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

    fn eat_word(&mut self, w: &str) -> bool {
        if self.word_is(w) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn word_is(&self, w: &str) -> bool {
        self.peek() == Some(&Tok::Word(w.to_string()))
    }

    fn bail<T>(&self, why: &str) -> Result<T, String> {
        Err(format!("条件不支持：{why}（token {:?}）", self.peek()))
    }

    /// `or` 优先级最低。
    fn parse_expr(&mut self) -> Result<Condition, String> {
        let mut left = self.parse_and()?;
        while self.word_is("or") {
            self.pos += 1;
            let right = self.parse_and()?;
            left = match left {
                Condition::Or(mut v) => {
                    v.push(right);
                    Condition::Or(v)
                }
                other => Condition::Or(vec![other, right]),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Condition, String> {
        let mut left = self.parse_unit()?;
        while self.word_is("and") {
            self.pos += 1;
            let right = self.parse_unit()?;
            left = match left {
                Condition::And(mut v) => {
                    v.push(right);
                    Condition::And(v)
                }
                other => Condition::And(vec![other, right]),
            };
        }
        Ok(left)
    }

    fn parse_unit(&mut self) -> Result<Condition, String> {
        if self.word_is("not") {
            self.pos += 1;
            let inner = self.parse_unit()?;
            return Ok(Condition::Not(Box::new(inner)));
        }
        if self.eat_word("any") {
            if !self.eat_word("of") {
                return self.bail("any 后缺 of");
            }
            return self.parse_of_tail(true, None);
        }
        if self.eat_word("all") {
            if !self.eat_word("of") {
                return self.bail("all 后缺 of");
            }
            return self.parse_of_tail(false, None);
        }
        if self.eat_sym('(') {
            let inner = self.parse_expr()?;
            if !self.eat_sym(')') {
                return self.bail("括号未闭合");
            }
            return Ok(inner);
        }
        // N of … / $ref …
        if let Some(Tok::Word(w)) = self.peek().cloned() {
            if let Ok(n) = w.parse::<usize>() {
                self.pos += 1;
                if !self.eat_word("of") {
                    return self.bail("数字后只支持 of（比较/算术不支持）");
                }
                return self.parse_of_tail(false, Some(n));
            }
            if let Some(id) = w.strip_prefix('$') {
                self.pos += 1;
                return self.parse_ref_tail(id);
            }
        }
        self.bail("无法识别的条件起点")
    }

    /// `of` 之后的尾巴（调用方已消费 `of`）：`them` 或 `($x*)`；
    /// `any` → OfAny、`all` → OfAll、`N` → OfCount。
    fn parse_of_tail(&mut self, any: bool, n: Option<usize>) -> Result<Condition, String> {
        let prefix = if self.eat_word("them") {
            None
        } else {
            if !self.eat_sym('(') {
                return self.bail("of 后缺 them 或 (");
            }
            let Some(Tok::Word(w)) = self.next() else {
                return self.bail("of ( 后缺串引用");
            };
            let Some(id) = w.strip_prefix('$') else {
                return self.bail("of ( 后缺 $ 引用");
            };
            let prefix = id.strip_suffix('*').map(|p| p.to_string());
            if !self.eat_sym(')') {
                return self.bail("串集合未闭合");
            }
            prefix
        };
        Ok(match (any, n) {
            (true, _) => Condition::OfAny { prefix },
            (false, None) => Condition::OfAll { prefix },
            (false, Some(n)) => Condition::OfCount { n, prefix },
        })
    }

    /// `$id` 尾巴：裸引用 / `at N` / `in (a .. b)`。
    fn parse_ref_tail(&mut self, id: &str) -> Result<Condition, String> {
        let index = match self.strings.iter().position(|(sid, _)| sid == id) {
            Some(i) => i,
            None => return self.bail(&format!("引用了未声明的串 ${id}")),
        };
        if self.eat_word("at") {
            let offset = self.parse_int()?;
            return Ok(Condition::At { index, offset });
        }
        if self.eat_word("in") {
            if !self.eat_sym('(') {
                return self.bail("in 后缺 (");
            }
            let lo = self.parse_int()?;
            // 区间分隔符：`..` 在词法层被 '.' 断字拆成两个 Sym('.')，
            // 带空格的 `..` 也可能保持 Word 形态——两种都认。
            let sep_ok = if self.peek() == Some(&Tok::Sym('.')) {
                self.pos += 1;
                self.eat_sym('.')
            } else if self.word_is("..") {
                self.pos += 1;
                true
            } else {
                false
            };
            if !sep_ok {
                return self.bail("in 区间缺 ..");
            }
            let hi = self.parse_int()?;
            if !self.eat_sym(')') {
                return self.bail("in 区间未闭合");
            }
            return Ok(Condition::In { index, lo, hi });
        }
        Ok(Condition::Ref(index))
    }

    /// 整数字面量（十进制 / 0x 十六进制）。算术表达式不支持（fail-closed）。
    fn parse_int(&mut self) -> Result<u64, String> {
        let Some(Tok::Word(w)) = self.next() else {
            return self.bail("缺整数");
        };
        if let Some(hex) = w.strip_prefix("0x") {
            return u64::from_str_radix(hex, 16).map_err(|_| format!("非法十六进制 {w:?}"));
        }
        w.parse::<u64>().map_err(|_| format!("非法整数 {w:?}"))
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
