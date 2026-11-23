//! 诊断、悬停、补全、文档符号。
//!
//! 语法与语义全部复用 `trit-core` / `trit-eval`：本模块只负责把位置换成 LSP 的
//! 行列、把运算符元数据渲染成 markdown。

use serde_json::{Value, json};
use trit_core::value::Ternary;
use trit_core::{Algebra, Expr, Parser as ExprParser};
use trit_eval::eval::{Assignment, eval};

use crate::lex::{self, Kind, Token};
use crate::text::{LineIndex, Position, Range};

/// LSP 诊断严重级：错误（既有诊断用这个）。
pub const SEV_ERROR: u8 = 1;
/// LSP 诊断严重级：警告（深层三值 lint 用这个）。
pub const SEV_WARNING: u8 = 2;
/// LSP 诊断严重级：提示（语义分歧用这个）。
pub const SEV_INFORMATION: u8 = 3;

/// 一条诊断（字节区间）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// 给人看的消息。
    pub message: String,
    /// 起始字节。
    pub start: usize,
    /// 结束字节（不含）。
    pub end: usize,
    /// LSP severity（1=Error、2=Warning、3=Information）。
    pub severity: u8,
}

impl Diagnostic {
    /// 错误级诊断（`trit build` 前端同款规则）。
    fn error(message: impl Into<String>, start: usize, end: usize) -> Self {
        Self {
            message: message.into(),
            start,
            end,
            severity: SEV_ERROR,
        }
    }
}

/// 一次分析的产物。
pub struct Analysis {
    /// 全部 token。
    pub tokens: Vec<Token>,
    /// 诊断。
    pub diagnostics: Vec<Diagnostic>,
    /// 变量首次出现（去重、按出现顺序）。
    pub variables: Vec<Token>,
    /// 解析成功时的表达式。
    pub expr: Option<Expr>,
}

/// 分析整篇文档。
///
/// 规则与 `trit build` 的前端一致：只允许一条表达式（`#` 注释与空行忽略）。
pub fn analyze(text: &str) -> Analysis {
    let idx = LineIndex::new(text);
    let mut diagnostics = Vec::new();
    let mut expression: Option<(usize, usize, String)> = None; // (行, 起始字节, 原文)

    for line in 0..idx.line_count() {
        let start = idx.line_start(line).unwrap_or(text.len());
        let end = idx.line_start(line + 1).unwrap_or(text.len());
        let raw = &text[start..end];
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let leading = raw.len() - raw.trim_start().len();
        let begin = start + leading;
        if expression.is_some() {
            diagnostics.push(Diagnostic::error(
                "一个 .tri 文件只能有一条表达式（多行表达式暂不支持）",
                begin,
                begin + trimmed.len(),
            ));
        } else {
            expression = Some((line as usize, begin, trimmed.to_string()));
        }
    }

    let tokens = lex::tokens(text);
    let variables = collect_variables(&tokens);

    let mut expr = None;
    match expression {
        None => diagnostics.push(Diagnostic::error("文件里没有表达式", 0, 0)),
        Some((_, base, src)) => match ExprParser::parse(&src) {
            Ok(parsed) => {
                // 深层三值诊断：布尔化简陷阱（全文档 token，多行文件的额外
                // 表达式行同样要报）+ 首条表达式的三语义对拍。
                diagnostics.extend(crate::semantics::law_lints(&tokens));
                if let Some(message) = crate::semantics::semantic_divergence(&parsed) {
                    diagnostics.push(Diagnostic {
                        message,
                        severity: SEV_INFORMATION,
                        start: base,
                        end: base + src.len(),
                    });
                }
                expr = Some(parsed);
            }
            Err(err) => {
                let (start, end) = error_span(&err, &src, base);
                diagnostics.push(Diagnostic::error(err.to_string(), start, end));
            }
        },
    }

    Analysis {
        tokens,
        diagnostics,
        variables,
        expr,
    }
}

fn collect_variables(tokens: &[Token]) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    for token in tokens.iter().filter(|t| t.kind == Kind::Ident) {
        if !out.iter().any(|t| t.text == token.text) {
            out.push(token.clone());
        }
    }
    out
}

/// 把解析错误（1 起的行/列）换成整篇文档里的字节区间。
fn error_span(err: &trit_core::parser::ParseError, src: &str, base: usize) -> (usize, usize) {
    use trit_core::parser::ParseError as E;
    let (line, column) = match err {
        E::UnexpectedEof { line, column }
        | E::UnexpectedChar { line, column, .. }
        | E::UnclosedParen { line, column } => (*line, *column),
        E::InvalidIdentifier { line, column, name } => {
            let offset = offset_at(src, *line, *column);
            let start = base + offset;
            let end = (base + offset + name.len().max(1)).min(base + src.len());
            return (start, end.max(start));
        }
        // 无位置信息的预算类错误：标在整条表达式上。
        _ => return (base, base + src.len()),
    };
    let offset = offset_at(src, line, column);
    // `offset_at` 经 `char_indices` 取边界，`start` 随之落在字符边界上。
    let start = base + offset;
    // 宽度 = 该偏移处字符的 UTF-8 字节长度（不再硬编码 1 字节）；偏移已到
    // 文末（无字符）时宽度为 0，退化为 `(start, start)` 空 span。
    let width = src[offset..].chars().next().map_or(0, char::len_utf8);
    let mut end = start + width;
    end = end.min(base + src.len());
    // 兜底：`end` 必须是字符边界（宽度取 `len_utf8` 时天然是下一边界；这里
    // 防止任何偏移计算把 `end` 留在字符内部），并保持 `start <= end <= src.len()`。
    while end < base + src.len() && !src.is_char_boundary(end - base) {
        end += 1;
    }
    (start, end.max(start))
}

/// 1 起行列 → 字节偏移（行内列按字符计，与 `trit-core` 解析器一致）。
fn offset_at(src: &str, line: usize, column: usize) -> usize {
    let mut current_line = 1usize;
    let mut line_start = 0usize;
    for (i, ch) in src.char_indices() {
        if current_line == line {
            line_start = i;
            break;
        }
        if ch == '\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }
    if current_line != line {
        return src.len();
    }
    src[line_start..]
        .char_indices()
        .nth(column.saturating_sub(1))
        .map(|(i, _)| line_start + i)
        .unwrap_or(src.len())
}

/// 悬停：返回 markdown 与 token 的范围。
pub fn hover(
    text: &str,
    pos: Position,
    algebra: Algebra,
    analysis: &Analysis,
) -> Option<(String, Range)> {
    let idx = LineIndex::new(text);
    let offset = idx.offset(text, pos);
    let token = analysis
        .tokens
        .iter()
        .find(|t| t.start <= offset && offset < t.end)?;
    let markdown = match token.kind {
        Kind::Operator => {
            let info = lex::operator(&token.text)?;
            render_operator(info, algebra)
        }
        Kind::Ident => format!(
            "**变量** `{}`\n\n`.tri` 是单表达式文件，变量在文件内局部。",
            token.text
        ),
        Kind::Constant => {
            let value = constant(&token.text)?;
            format!("**常量** `{}`（{}）", token.text, describe(value))
        }
        Kind::LParen | Kind::RParen => "**分组** `(...)`：改变运算优先级。".to_string(),
        Kind::Unknown => return None,
    };
    Some((markdown, idx.range(text, token.start, token.end)))
}

/// 运算符的 markdown：名称、优先级、结合性、当前语义下的 3×3 真值表。
pub fn render_operator(info: &lex::OpInfo, algebra: Algebra) -> String {
    format!(
        "**{}** `{}`\n\n- 优先级 {}（`!` 最高，`<=>` 最低）\n- {}\n- 语义：{}\n\n{}",
        info.name,
        info.primary(),
        info.precedence,
        info.assoc,
        algebra.as_str(),
        truth_table(info.sample, info.arity, algebra)
    )
}

/// 用 `trit-eval` 现算真值表——语义只有一份，不在 LSP 里抄第二份。
fn truth_table(sample: &str, arity: u8, algebra: Algebra) -> String {
    let Ok(expr) = ExprParser::parse(sample) else {
        return "（样例解析失败，略）".to_string();
    };
    let values = [Ternary::False, Ternary::Unknown, Ternary::True];
    let mut out = String::new();
    if arity == 1 {
        out.push_str("| a | 结果 |\n|---|---|\n");
        for a in values {
            let mut assignment = Assignment::new();
            assignment.set("a", a);
            let value = eval(&expr, &assignment, algebra);
            out.push_str(&format!("| {} | {} |\n", a.as_char(), value.as_char()));
        }
        return out;
    }
    out.push_str("| a \\ b | 0 | X | 1 |\n|---|---|---|---|\n");
    for a in values {
        out.push_str(&format!("| {} |", a.as_char()));
        for b in values {
            let mut assignment = Assignment::new();
            assignment.set("a", a);
            assignment.set("b", b);
            let value = eval(&expr, &assignment, algebra);
            out.push_str(&format!(" {} |", value.as_char()));
        }
        out.push('\n');
    }
    out
}

fn describe(value: Ternary) -> &'static str {
    match value {
        Ternary::False => "假",
        Ternary::Unknown => "未知",
        Ternary::True => "真",
    }
}

/// 补全项：运算符、常量词、本文件变量。
pub fn completion(analysis: &Analysis) -> Vec<Value> {
    let mut items = Vec::new();
    for op in lex::OPS {
        items.push(json!({
            "label": op.primary(),
            "kind": 24,
            "detail": format!("{}（优先级 {}，{}）", op.name, op.precedence, op.assoc),
            "insertText": op.primary(),
            "documentation": format!("其他拼写：{}", op.spellings.join("  ")),
        }));
    }
    for (word, desc) in [
        ("True", "真"),
        ("False", "假"),
        ("X", "未知"),
        ("0", "假"),
        ("1", "真"),
        ("?", "未知"),
    ] {
        items.push(json!({
            "label": word,
            "kind": 14,
            "detail": format!("常量（{desc}）"),
            "insertText": word,
        }));
    }
    for variable in &analysis.variables {
        items.push(json!({
            "label": variable.text,
            "kind": 6,
            "detail": "变量（本文件）",
            "insertText": variable.text,
        }));
    }
    items
}

/// 文档符号：变量（DocumentSymbol kind 13）。
pub fn symbols(text: &str, analysis: &Analysis) -> Vec<Value> {
    let idx = LineIndex::new(text);
    let whole = idx.whole(text).to_json();
    analysis
        .variables
        .iter()
        .map(|variable| {
            json!({
                "name": variable.text,
                "kind": 13,
                "range": whole,
                "selectionRange": idx.range(text, variable.start, variable.end).to_json(),
            })
        })
        .collect()
}

/// 诊断转 LSP JSON。
pub fn diagnostics_json(text: &str, analysis: &Analysis) -> Vec<Value> {
    let idx = LineIndex::new(text);
    analysis
        .diagnostics
        .iter()
        .map(|d| {
            json!({
                "range": idx.range(text, d.start, d.end).to_json(),
                "severity": d.severity,
                "source": "trit-lsp",
                "message": d.message,
            })
        })
        .collect()
}

/// 常量词 → 三值（`0/1/?`、`true/false/X` 等）。
pub(crate) fn constant(text: &str) -> Option<Ternary> {
    match text {
        "0" | "False" | "false" | "FALSE" => Some(Ternary::False),
        "1" | "True" | "true" | "TRUE" => Some(Ternary::True),
        "?" | "X" | "x" | "Unknown" | "unknown" => Some(Ternary::Unknown),
        _ => None,
    }
}

/// 是不是变量标识符（`Ident` 且不是常量词）。
pub(crate) fn is_variable(token: &Token) -> bool {
    token.kind == Kind::Ident && constant(&token.text).is_none()
}

#[cfg(test)]
#[path = "analysis_tests.rs"]
mod tests;
