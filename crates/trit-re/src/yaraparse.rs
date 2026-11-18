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
//!   `xor`/`xor(k)`/`xor(a-b)` / `private`）、`$id = { hex 串 }`（通配/跳跃同
//!   [`crate::sigscan::Pattern::hex`]）；正则串与 `base64`/`base64wide` 修饰
//!   **可解析不可扫描**：串记为 [`crate::sigscan::Pattern::Unscannable`]，
//!   整条规则扫描时判 `Unknown`；
//! - 条件表达式：`and/or/not`、括号、`$id`、`$id at N`、`$id in (a..b)`、
//!   `any/all/N of them`、`any/all/N of ($x*)`、`true`/`false`；
//! - **数值比较**：`filesize`、`uint8/16/32(lit)`、`#a`（计数）、`@a[N]`
//!   （偏移，**下标 1 起**——官方 YARA 语义，yara-python 实测判定）与
//!   `< <= > >= == !=`（KB/MB 后缀字面量同认）；
//! - **for-of 循环**：`for N|any|all of them/($x*) : ( 体 )`，体内 `$`、
//!   `#$`、`@$`/`@$[N]` 绑定当前串；
//! - **模块属性**：`pe.*` 标量、`pe.sections[N].*`、`pe.rich_signature.*`
//!   （越界/无 Rich = 确定的未定义）与 `.name`/rich 字符串 `==`/`!=` 求值；
//!   `math.*` 等子集外路径记名保留、求值恒 `Unknown`。`private` 串匹配
//!   照常、仅报告隐藏。`entrypoint` = 入口点**文件偏移**（官方 YARA 语义，
//!   实测对拍判定）——CLI 从 PE 头解析传入
//!   [`crate::sigscan::SigScan::scan_with_entrypoint`]，库直调不传上下文时弃权；
//! - 注释：`//` 行注释与 `/* */` 块注释（词法器识别引号内的注释字符——
//!   字符串里出现 `//` 是合法内容）。
//!
//! ## 表达式语言（`xparse`，见 `docs/YARA-EXPR.md`）
//!
//! 上面的旧条件解析器接不住的规则（算术 / 位运算、`for … in`、字符串函数、
//! pe 对象路径）转交表达式引擎；两边都拒绝才是 `Unsupported`（带原因）。
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

use crate::sigscan::Rule;

mod cond; // 条件表达式解析（拆出守预算）；`#`/`@` 叶子在 cond_tail
mod cond_cmp;
mod cond_leaf;
mod cond_tail;
mod file_loop;
mod rule_body;
mod rulebuild;
mod xparse; // 表达式语言：旧解析器接不住的条件由它接

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
    Parser::with_conditions(lex(text)?, text).parse_file()
}

mod lex;
mod valuemod; // 串值解析与修饰符消费（从本文件拆出守预算）

use lex::{Tok, lex};

// ── 解析 ────────────────────────────────────────────────────────────────

struct Parser {
    pub(super) toks: Vec<Tok>,
    pub(super) pos: usize,
    /// 表达式引擎用的条件视图（按规则名取）。
    pub(super) xconds: xparse::Conditions,
}

impl Parser {
    pub(super) fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    pub(super) fn next(&mut self) -> Option<Tok> {
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
