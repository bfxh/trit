//! 顶层规则循环（从 `yaraparse.rs` 拆出守预算）：单条规则的错误隔离。
//!
//! 官方把一条规则的编译错误限制在该规则内；这里同样只把出错的规则记
//! `Unsupported`，跳到下一个顶层 `rule` 继续。

use super::{ParsedFile, Parser, Tok};

impl Parser {
    pub(super) fn parse_file(&mut self) -> Result<ParsedFile, String> {
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
}
