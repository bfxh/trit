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
        meta_keys: usize,
        private_ids: Vec<usize>,
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
        Ok(RuleOutcome::Parsed(Rule::from_parts(
            name,
            strings,
            cond,
            private_ids,
        )))
    }
}
