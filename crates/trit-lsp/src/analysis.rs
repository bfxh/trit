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

/// 一条诊断（字节区间）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// 给人看的消息。
    pub message: String,
    /// 起始字节。
    pub start: usize,
    /// 结束字节（不含）。
    pub end: usize,
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
            diagnostics.push(Diagnostic {
                message: "一个 .tri 文件只能有一条表达式（多行表达式暂不支持）".into(),
                start: begin,
                end: begin + trimmed.len(),
            });
        } else {
            expression = Some((line as usize, begin, trimmed.to_string()));
        }
    }

    let tokens = lex::tokens(text);
    let variables = collect_variables(&tokens);

    let mut expr = None;
    match expression {
        None => diagnostics.push(Diagnostic {
            message: "文件里没有表达式".into(),
            start: 0,
            end: 0,
        }),
        Some((_, base, src)) => match ExprParser::parse(&src) {
            Ok(parsed) => expr = Some(parsed),
            Err(err) => {
                let (start, end) = error_span(&err, &src, base);
                diagnostics.push(Diagnostic {
                    message: err.to_string(),
                    start,
                    end,
                });
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
    let (line, column, width) = match err {
        E::UnexpectedEof { line, column }
        | E::UnexpectedChar { line, column, .. }
        | E::UnclosedParen { line, column } => (*line, *column, 1),
        E::InvalidIdentifier { line, column, name } => (*line, *column, name.len().max(1)),
        // 无位置信息的预算类错误：标在整条表达式上。
        _ => return (base, base + src.len()),
    };
    let offset = offset_at(src, line, column);
    let start = base + offset;
    let end = (start + width).min(base + src.len()).max(start);
    (start, end)
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

fn constant(text: &str) -> Option<Ternary> {
    match text {
        "0" | "False" | "false" | "FALSE" => Some(Ternary::False),
        "1" | "True" | "true" | "TRUE" => Some(Ternary::True),
        "?" | "X" | "x" | "Unknown" | "unknown" => Some(Ternary::Unknown),
        _ => None,
    }
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
                "severity": 1,
                "source": "trit-lsp",
                "message": d.message,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_expression_has_no_diagnostics() {
        let a = analyze("# demo\n(a & b) | !c\n");
        assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
        assert!(a.expr.is_some());
        let names: Vec<&str> = a.variables.iter().map(|v| v.text.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn empty_file_reports() {
        let a = analyze("# only comment\n\n");
        assert_eq!(a.diagnostics.len(), 1);
        assert!(a.diagnostics[0].message.contains("没有表达式"));
    }

    #[test]
    fn two_expressions_report_on_second_line() {
        let text = "a & b\nc | d\n";
        let a = analyze(text);
        assert_eq!(a.diagnostics.len(), 1);
        assert!(a.diagnostics[0].message.contains("只能有一条表达式"));
        assert_eq!(a.diagnostics[0].start, 6);
    }

    #[test]
    fn parse_error_has_range() {
        let text = "# x\na & )\n";
        let a = analyze(text);
        assert_eq!(a.diagnostics.len(), 1);
        assert!(a.diagnostics[0].message.contains("无法识别"));
        let idx = LineIndex::new(text);
        let range = idx.range(text, a.diagnostics[0].start, a.diagnostics[0].end);
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 4);
    }

    #[test]
    fn hover_operator_renders_truth_table() {
        let text = "a & b";
        let a = analyze(text);
        let (markdown, range) =
            hover(text, Position::new(0, 2), Algebra::Kleene, &a).expect("有悬停");
        assert!(markdown.contains("与（合取）"));
        assert!(markdown.contains("| a \\ b |"));
        assert!(markdown.contains("| 0 | 0 | 0 | 0 |"));
        assert_eq!(range.start, Position::new(0, 2));
        assert_eq!(range.end, Position::new(0, 3));
    }

    #[test]
    fn hover_variable() {
        let text = "a & b";
        let a = analyze(text);
        let (markdown, _) = hover(text, Position::new(0, 0), Algebra::Kleene, &a).expect("有悬停");
        assert!(markdown.contains("变量"));
    }

    #[test]
    fn hover_differs_by_algebra() {
        let kleene = render_operator(lex::operator("&").expect("有"), Algebra::Kleene);
        let bochvar = render_operator(lex::operator("&").expect("有"), Algebra::Bochvar);
        assert!(kleene.contains("X"));
        assert_ne!(kleene, bochvar);
    }

    #[test]
    fn completion_lists_ops_constants_and_variables() {
        let a = analyze("a & b");
        let items = completion(&a);
        let labels: Vec<&str> = items
            .iter()
            .filter_map(|i| i.get("label").and_then(Value::as_str))
            .collect();
        assert!(labels.contains(&"&"));
        assert!(labels.contains(&"X"));
        assert!(labels.contains(&"a"));
        assert!(labels.contains(&"b"));
    }

    #[test]
    fn symbols_are_variables() {
        let text = "a & b";
        let a = analyze(text);
        let symbols = symbols(text, &a);
        let names: Vec<&str> = symbols
            .iter()
            .filter_map(|s| s.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn diagnostics_json_has_source_and_range() {
        let text = "a & )";
        let a = analyze(text);
        let values = diagnostics_json(text, &a);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["severity"], 1);
        assert_eq!(values[0]["source"], "trit-lsp");
        assert!(values[0]["range"]["start"]["line"].is_number());
    }
}
