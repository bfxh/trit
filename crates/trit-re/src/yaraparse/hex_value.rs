//! hex 块解析（从 `valuemod.rs` 拆出守模块预算）。

use super::{Parser, Tok};
use crate::sigscan::Pattern;

impl Parser {
    /// 吃 `{ ... }` hex 块到 `}`，token 拼回空格分隔的 hex spec。
    /// 段数超限记 `unsupported` 并返回 `Ok(None)`（整条规则弃权，不 abort、
    /// 不当命中）；其余 hex 语法错误按整文件解析失败上报。
    pub(super) fn parse_hex_block(
        &mut self,
        unsupported: &mut Option<String>,
        rule_name: &str,
    ) -> Result<Option<Pattern>, String> {
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
            Ok(p) => Ok(Some(p)),
            Err(e) if e.starts_with("hex 段数") => {
                let _ = unsupported.insert(e);
                Ok(None)
            }
            Err(e) => Err(format!("规则 {rule_name} hex 串不合法：{e}")),
        }
    }
}
