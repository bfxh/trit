//! `#` / `@` 操作数叶子解析（从 cond.rs 拆出——模块预算棘轮纪律：
//! 拆文件不抬台账）。
//!
//! 官方语义实测判定：`@a[N]` 下标 **1 起**（`@a[1]` = 首次、`@a[0]`
//! 无效）；`#`/`@$`/`@$[2]`（当前串形态）只在 for-of 体内合法。

use super::cond::CondParser;
use crate::sigscan::Operand;

impl CondParser<'_> {
    /// `#` 后的串名：`#a`（计数）/ `#$` / 裸 `#`（for-of 体内当前串）。
    pub(super) fn count_operand(&mut self, rest: &str) -> Result<Operand, String> {
        if rest.is_empty() || rest == "$" {
            if self.for_depth == 0 {
                return self.bail("'#' 只能在 for-of 体内使用");
            }
            return Ok(Operand::CurrentCount);
        }
        let index = self.string_index(rest)?;
        Ok(Operand::Count(index))
    }

    /// `@` 后的串名与可选 `[N]`：`@a` / `@a[2]` / `@$` / 裸 `@$[2]`。
    /// **下标 1 起**（官方 YARA 语义，yara-python 实测判定：`@a[1]` =
    /// 首次、`@a[0]` 无效）——解析成 0 起的内部下标。
    pub(super) fn offset_operand(&mut self, rest: &str) -> Result<Operand, String> {
        let current = rest.is_empty() || rest == "$";
        let mut nth = 0usize;
        if self.eat_sym('[') {
            let n = self.parse_int()? as usize;
            if n == 0 {
                return self.bail("@ 下标从 1 起（官方 YARA 语义）");
            }
            nth = n - 1;
            if !self.eat_sym(']') {
                return self.bail("@ 偏移下标未闭合");
            }
        }
        if current {
            if self.for_depth == 0 {
                return self.bail("'@' 只能在 for-of 体内使用");
            }
            return Ok(Operand::CurrentOffset { nth });
        }
        let index = self.string_index(rest)?;
        Ok(Operand::Offset { index, nth })
    }
}
