//! 表达式语言解析入口（N4 深水区）：把旧条件解析器（`cond.rs`）接不住的
//! 规则交给新的表达式引擎，旧子集的既有判据**一字不动**。
//!
//! 分流规则（[`route`]）：
//! 1. 条件里出现表达式语言的标志（`+ \ % & | ^ ~ << >>`、字符串函数、
//!    `defined`、`for … in`）→ 只走新引擎；它拒绝就是 `Unsupported`。
//! 2. 否则旧解析器优先——它**真能求值**的规则判据一字不动。
//! 3. 旧解析器拒绝的（负数字面量、`none of`、`int8` 之类）让新引擎再试
//!    一次；两边都拒绝则两个原因一并上报。
//! 4. 旧解析器"接住"却只能记名弃权的（条件里有它永远求不出值的模块
//!    路径 / 函数，或它做不了的字符串比较——字符串之间比大小、字符串
//!    对非字符串，见 [`legacy`]）也让新引擎试：能解析就换新引擎
//!    （`Unknown` → 确定结论，只增证据面）；不能就维持旧的记名弃权。
//! 5. 旧解析器接住、但条件里有 `not` 罩着可能未定义的比较
//!    （`not (uint8(越界) == 1)`、`not (@a[9] > 0)`）：旧求值器把未定义
//!    当假再取反，官方的 `not 未定义` 仍是未定义——与官方对拍实测有
//!    DIFF，所以交给新引擎；新引擎不收时维持旧结果（见 YARA-EXPR.md）。
//! 6. 条件里有 `\xNN` / `\r` 转义的字符串字面量：旧词法层会把它读错
//!    （`"\x41"` 读成 `x41`），同样先让新引擎试。
//! 7. 旧解析器接住了 `for … of` 套 `for … of`：官方是编译错误，旧求值器
//!    却会给出命中——交给新引擎，由它照官方拒绝（与 `for … in` 混套合法）。
//! 8. 模块数组的字面量下标超出 C `int`（`pe.sections[4294967296]`）：官方
//!    截成 32 位再用，旧求值器判"越界"——交给新引擎照官方截断。
//!
//! 官方编译错误 → 整条规则 `Unsupported`，绝不静默跳过：除零、常量溢出、
//! 非法移位、类型不匹配、未知字段、重复循环变量、循环嵌套超限等，逐条
//! 对应官方 4.5.4 的编译期检查（见 `docs/YARA-EXPR.md`）。

use super::{Parser, Tok};
use crate::sigscan::{Condition, Pattern};
use std::collections::{BTreeMap, BTreeSet};

mod boolean;
mod fold;
mod legacy;
mod lex;
mod path;
mod primary;
mod sets;

use lex::T;

/// 规则文件的表达式视图：每条规则的条件 token + 文件级 `import` 列表。
#[derive(Debug, Default)]
pub(super) struct Conditions {
    rules: BTreeMap<String, Vec<T>>,
    /// 条件里有旧词法层会读错的字符串字面量（`\xNN` / `\r`）的规则。
    misread: BTreeSet<String>,
    imports: Vec<Vec<u8>>,
    /// 新词法层整体失败的原因（此时所有规则只能走旧解析器）。
    lex_error: Option<String>,
}

impl Conditions {
    /// 在原文上重新切词并按规则名取出条件段（同名规则取第一条——重名
    /// 本身是官方编译错误）。
    pub(super) fn scan(text: &str) -> Self {
        let tokens = match lex::lex(text) {
            Ok(tokens) => tokens,
            Err(reason) => {
                return Self {
                    lex_error: Some(reason),
                    ..Self::default()
                };
            }
        };
        let mut out = Self::default();
        let mut at = 0;
        while at < tokens.len() {
            match (&tokens[at], tokens.get(at + 1)) {
                (T::Ident(word), Some(T::Str(module))) if word == "import" => {
                    out.imports.push(module.clone());
                    at += 2;
                }
                (T::Ident(word), Some(T::Ident(name))) if word == "rule" => {
                    let (mut condition, next) = rule_condition(&tokens, at + 2);
                    for token in &mut condition {
                        if let T::RawStr(bytes) = token {
                            *token = T::Str(std::mem::take(bytes));
                            out.misread.insert(name.clone());
                        }
                    }
                    out.rules.entry(name.clone()).or_insert(condition);
                    at = next;
                }
                _ => at += 1,
            }
        }
        out
    }
}

/// 从规则名之后扫到规则体收尾，取出 `condition:` 之后的 token。
fn rule_condition(tokens: &[T], mut at: usize) -> (Vec<T>, usize) {
    while at < tokens.len() && tokens[at] != T::P("{") {
        at += 1;
    }
    let (mut depth, mut condition, mut inside) = (0usize, Vec::new(), false);
    while at < tokens.len() {
        let token = &tokens[at];
        at += 1;
        match token {
            T::P("{") => depth += 1,
            T::P("}") => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            T::Ident(word)
                if word == "condition" && depth == 1 && tokens.get(at) == Some(&T::P(":")) =>
            {
                inside = true;
                at += 1;
                continue;
            }
            _ => {}
        }
        if inside {
            condition.push(token.clone());
        }
    }
    (condition, at)
}

/// 条件里有没有"只有表达式引擎才该接"的标志。
fn wants_expr(tokens: &[T]) -> bool {
    const OPERATORS: &[&str] = &["+", "\\", "%", "&", "|", "^", "~", "<<", ">>"];
    const WORDS: &[&str] = &[
        "contains",
        "icontains",
        "startswith",
        "istartswith",
        "endswith",
        "iendswith",
        "iequals",
        "matches",
        "defined",
    ];
    tokens.iter().enumerate().any(|(at, token)| match token {
        T::P(p) => OPERATORS.contains(p),
        T::Ident(word) if WORDS.contains(&word.as_str()) => true,
        // `for <量词> <变量> in …`：冒号之前没有 `of` 就是变量循环
        T::Ident(word) if word == "for" => !tokens[at..]
            .iter()
            .take_while(|t| **t != T::P(":"))
            .any(|t| matches!(t, T::Ident(w) if w == "of")),
        _ => false,
    })
}

impl Parser {
    /// 旧词法流 + 表达式引擎的条件视图（两套词法各扫一遍源文本）。
    pub(super) fn with_conditions(toks: Vec<Tok>, text: &str) -> Self {
        let xconds = Conditions::scan(text);
        Self {
            toks,
            pos: 0,
            xconds,
        }
    }
}

/// 旧条件解析器是没有深度闸的递归下降：括号 / `not` 叠上几千层就爆栈
/// （进程直接 abort，实测 2000 层括号即可）。进旧解析器之前先数一遍
/// 嵌套，超过表达式引擎同款上限的条件两边都不收。
pub(super) fn guard_depth(tokens: &[Tok]) -> Result<(), String> {
    let (mut open, mut prefixes) = (0u32, 0u32);
    for token in tokens {
        match token {
            Tok::Sym('(' | '[') => open += 1,
            Tok::Sym(')' | ']') => open = open.saturating_sub(1),
            Tok::Word(word) if word == "not" => prefixes += 1,
            _ => prefixes = 0,
        }
        if open + prefixes > boolean::MAX_NESTING {
            return Err(format!("嵌套过深（上限 {} 层）", boolean::MAX_NESTING));
        }
    }
    Ok(())
}

/// 分流：见模块文档。`legacy` 是旧条件解析器的结局。
pub(super) fn route(
    parser: &Parser,
    name: &str,
    strings: &[(String, Pattern)],
    legacy: Result<Condition, String>,
) -> Result<Condition, String> {
    let view = &parser.xconds;
    let Some(tokens) = view.rules.get(name) else {
        return legacy.map_err(|reason| match &view.lex_error {
            Some(lex) => format!("{reason}；表达式引擎词法失败：{lex}"),
            None => reason,
        });
    };
    let ids: Vec<&str> = strings.iter().map(|(id, _)| id.as_str()).collect();
    let parse = || {
        let has_pe = view.imports.iter().any(|module| module == b"pe");
        boolean::Parser::new(tokens, &ids, has_pe)
            .parse_full()
            .map(|expr| Condition::Expr(Box::new(expr)))
    };
    if wants_expr(tokens) {
        return parse().map_err(|reason| format!("表达式不支持：{reason}"));
    }
    match legacy {
        // 官方拒绝、旧解析器却收下的写法：让表达式引擎给出拒绝理由
        Ok(old) if legacy::nests_string_loops(&old) => {
            parse().map_err(|reason| format!("表达式不支持：{reason}"))
        }
        Ok(old)
            if legacy::abstains_by_name(&old)
                || legacy::negates_undefined(&old)
                || view.misread.contains(name)
                || legacy::wraps_index(tokens) =>
        {
            Ok(parse().unwrap_or(old))
        }
        Ok(old) => Ok(old),
        Err(old) => parse().map_err(|new| format!("{old}；表达式引擎：{new}")),
    }
}

#[cfg(test)]
mod tests;
