//! 单条规则体解析（从 `yaraparse.rs` 拆出守预算）：`meta` / `strings` /
//! `condition` 三段、内容级错误隔离与规则收尾。

use super::{Parser, RuleOutcome, Tok};
use crate::sigscan::Pattern;

impl Parser {
    /// `rule <name> [: tag… ] { … }`
    pub(super) fn parse_rule(&mut self) -> Result<RuleOutcome, String> {
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
        let mut private_ids: Vec<usize> = Vec::new();
        let mut unsupported: Option<String> = None;
        let mut condition_toks: Vec<Tok> = Vec::new();

        while let Some(tok) = self.peek().cloned() {
            if tok == Tok::Sym('}') {
                self.pos += 1;
                if let Some(reason) = unsupported {
                    return Ok(RuleOutcome::Unsupported { name, reason });
                }
                return self.assemble_rule(name, strings, &condition_toks, meta_keys, private_ids);
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
                        let parsed = match self.parse_string_value(&mut unsupported, &name) {
                            Ok(v) => v,
                            Err(reason) => {
                                self.skip_to_rule_end();
                                return Ok(RuleOutcome::Unsupported { name, reason });
                            }
                        };
                        if let Some((pattern, private)) = parsed {
                            if private {
                                private_ids.push(strings.len());
                            }
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
                                if depth <= 0 {
                                    // 回退：'}' 留给外层循环（多余 ')' 不把深度带负）
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
                                depth = (depth - 1).max(0);
                                condition_toks.push(t);
                            }
                            other => condition_toks.push(other.clone()),
                        }
                    }
                }
                other => {
                    let reason = format!("规则 {name} 里出现意外 token：{other:?}");
                    self.skip_to_rule_end();
                    return Ok(RuleOutcome::Unsupported { name, reason });
                }
            }
        }
        Err(format!("规则 {name} 未闭合（缺 '}}'）"))
    }

    /// 内容级错误后跳到当前规则体的结束 `}`（结构级错误仍走 `Err`）。
    fn skip_to_rule_end(&mut self) {
        let mut depth = 1i32;
        while let Some(t) = self.next() {
            match t {
                Tok::Sym('{') => depth += 1,
                Tok::Sym('}') => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    fn peek_is_section_keyword(&self) -> bool {
        matches!(
            self.peek(),
            Some(Tok::Word(w)) if w == "strings" || w == "condition"
        )
    }
}
