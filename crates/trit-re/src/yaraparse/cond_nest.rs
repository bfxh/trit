//! 条件表达式的嵌套深度门与 `parse_full`（从 `cond.rs` 拆出，守行数预算）。

use super::cond::CondParser;
use crate::sigscan::Condition;

/// 条件表达式嵌套上限。解析器是递归下降：`not`、括号、for-of 体都会压栈。
/// 没有上限时，一份 16 KB 的 `((((…))))` 规则可以在 release 上栈溢出。
/// 128 层足够覆盖正常规则，同时给 debug 栈留出安全余量。
const MAX_COND_DEPTH: usize = 128;

impl<'a> CondParser<'a> {
    fn enter_nested(&mut self) -> Result<(), String> {
        if self.depth >= MAX_COND_DEPTH {
            return Err(format!(
                "条件嵌套超过上限 {MAX_COND_DEPTH} 层（拒绝递归下降栈溢出）"
            ));
        }
        self.depth += 1;
        Ok(())
    }

    pub(super) fn parse_nested_unit(&mut self) -> Result<Condition, String> {
        self.enter_nested()?;
        let result = self.parse_unit();
        self.depth -= 1;
        result
    }

    pub(super) fn parse_nested_expr(&mut self) -> Result<Condition, String> {
        self.enter_nested()?;
        let result = self.parse_expr();
        self.depth -= 1;
        result
    }

    /// 解析整个条件并检查 token 全部消化（残留 = 语法不支持，如实报错）。
    pub(super) fn parse_full(&mut self) -> Result<Condition, String> {
        let cond = self.parse_expr()?;
        if self.pos != self.toks.len() {
            return Err(format!(
                "条件尾部有未消化的 token：{:?}",
                self.toks.get(self.pos)
            ));
        }
        Ok(cond)
    }
}
