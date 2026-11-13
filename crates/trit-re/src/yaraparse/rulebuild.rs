//! 规则装配（从 `yaraparse.rs` 拆出守预算）：条件解析 + private 下标 +
//! `Rule` 构造。

use super::cond::CondParser;
use super::{Parser, RuleOutcome, Tok};
use crate::sigscan::{Pattern, Rule};

impl Parser {
    pub(super) fn assemble_rule(
        &self,
        name: String,
        strings: Vec<(String, Pattern)>,
        condition_toks: &[Tok],
        _meta_keys: usize,
        private_ids: Vec<usize>,
    ) -> Result<RuleOutcome, String> {
        // 嵌套过深的两个解析器都不收（递归下降会爆栈）；旧解析器接不住的转交表达式引擎。
        let cond = super::xparse::guard_depth(condition_toks).and_then(|()| {
            let legacy = CondParser::new(condition_toks, &strings).parse_full();
            super::xparse::route(self, &name, &strings, legacy)
        });
        let cond = match cond {
            Ok(c) => c,
            Err(reason) => return Ok(RuleOutcome::Unsupported { name, reason }),
        };
        Ok(RuleOutcome::Parsed(Rule::from_parts(
            name,
            strings,
            cond,
            private_ids,
        )))
    }
}
