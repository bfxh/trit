//! 变量导航：定义 / 引用 / 高亮 / 重命名。
//!
//! `.tri` 是单表达式文件，没有声明语句——「定义」就是变量的**首次出现**。
//! 位置换算全部走 [`crate::text::LineIndex`]（LSP 用 UTF-16 码元），
//! 与 hover / completion 同一套。

use serde_json::{Value, json};
use trit_core::ast::is_identifier;

use crate::analysis::{self, Analysis};
use crate::lex::{Kind, Token};
use crate::text::{LineIndex, Position, Range};

/// 光标处的 token（不在任何 token 上则 None）。
fn token_at<'a>(text: &str, pos: Position, analysis: &'a Analysis) -> Option<&'a Token> {
    let idx = LineIndex::new(text);
    let offset = idx.offset(text, pos);
    analysis
        .tokens
        .iter()
        .find(|t| t.start <= offset && offset < t.end)
}

/// 光标处的变量 token（常量词 `true`/`X` 与运算符都不算）。
fn variable_at<'a>(text: &str, pos: Position, analysis: &'a Analysis) -> Option<&'a Token> {
    let token = token_at(text, pos, analysis)?;
    (token.kind == Kind::Ident && analysis::is_variable(token)).then_some(token)
}

/// 变量的全部出现（按出现顺序）。
pub fn occurrences<'a>(analysis: &'a Analysis, name: &str) -> Vec<&'a Token> {
    analysis
        .tokens
        .iter()
        .filter(|t| t.kind == Kind::Ident && t.text == name && analysis::is_variable(t))
        .collect()
}

/// 转到定义：变量的首次出现；光标不在变量上时为 `None`（LSP 返回 null）。
pub fn definition(text: &str, pos: Position, analysis: &Analysis) -> Option<Range> {
    let token = variable_at(text, pos, analysis)?;
    let idx = LineIndex::new(text);
    let all = occurrences(analysis, &token.text);
    let first = all.first()?;
    Some(idx.range(text, first.start, first.end))
}

/// 查引用：变量的全部出现；`include_declaration` 决定含不含首次出现。
pub fn references(
    text: &str,
    pos: Position,
    include_declaration: bool,
    analysis: &Analysis,
) -> Vec<Range> {
    let Some(token) = variable_at(text, pos, analysis) else {
        return Vec::new();
    };
    let idx = LineIndex::new(text);
    occurrences(analysis, &token.text)
        .into_iter()
        .enumerate()
        .skip(if include_declaration { 0 } else { 1 })
        .map(|(_, t)| idx.range(text, t.start, t.end))
        .collect()
}

/// 文档高亮：变量的全部出现（LSP kind 1 = Text）。
pub fn highlight(text: &str, pos: Position, analysis: &Analysis) -> Vec<Value> {
    references(text, pos, true, analysis)
        .into_iter()
        .map(|range| json!({"range": range.to_json(), "kind": 1}))
        .collect()
}

/// 重命名：把变量的全部出现换成 `new_name`，返回 `WorkspaceEdit` 的 JSON。
///
/// 新名必须是合法标识符且不是常量词；光标不在变量上时为 `None`。
pub fn rename(
    text: &str,
    uri: &str,
    pos: Position,
    new_name: &str,
    analysis: &Analysis,
) -> Option<Value> {
    if !is_identifier(new_name) || analysis::constant(new_name).is_some() {
        return None;
    }
    let token = variable_at(text, pos, analysis)?;
    let idx = LineIndex::new(text);
    let edits: Vec<Value> = occurrences(analysis, &token.text)
        .into_iter()
        .map(|t| {
            json!({
                "range": idx.range(text, t.start, t.end).to_json(),
                "newText": new_name,
            })
        })
        .collect();
    Some(json!({"changes": {uri: edits}}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::text::Position;
    use serde_json::json;

    const TEXT: &str = "# demo\n(alpha & beta) | !alpha\n";
    const URI: &str = "file:///demo.tri";

    fn analysis() -> Analysis {
        analyze(TEXT)
    }

    /// 把字节区间换成 LSP Range 的 JSON，便于断言。
    fn range_json(start: usize, end: usize) -> Value {
        LineIndex::new(TEXT).range(TEXT, start, end).to_json()
    }

    #[test]
    fn definition_points_at_first_occurrence() {
        let a = analysis();
        // 第二个 alpha（第 2 行 `!alpha`，字符列 19）应跳回第一个（列 1）。
        let got = definition(TEXT, Position::new(1, 19), &a).expect("应找到定义");
        let first = TEXT.find("alpha").unwrap();
        assert_eq!(got.to_json(), range_json(first, first + 5));
    }

    #[test]
    fn references_skip_declaration_when_asked() {
        let a = analysis();
        assert_eq!(references(TEXT, Position::new(1, 19), true, &a).len(), 2);
        assert_eq!(references(TEXT, Position::new(1, 19), false, &a).len(), 1);
    }

    #[test]
    fn highlight_covers_all_occurrences() {
        let a = analysis();
        let items = highlight(TEXT, Position::new(1, 1), &a);
        assert_eq!(items.len(), 2, "alpha 出现两次：{items:?}");
        for item in &items {
            assert_eq!(item["kind"], 1);
        }
    }

    #[test]
    fn rename_replaces_all_occurrences() {
        let a = analysis();
        let edit = rename(TEXT, URI, Position::new(1, 1), "gamma", &a).expect("合法重命名");
        let changes = edit["changes"][URI].as_array().expect("changes 数组");
        assert_eq!(changes.len(), 2);
        // 套用两个编辑（任意顺序，区间互不重叠）后文本正确。
        let mut out = TEXT.as_bytes().to_vec();
        for item in changes {
            let start =
                LineIndex::new(TEXT).offset(TEXT, Position::from_json(&item["range"]["start"]));
            let end = LineIndex::new(TEXT).offset(TEXT, Position::from_json(&item["range"]["end"]));
            assert_eq!(&TEXT[start..end], "alpha");
            assert_eq!(item["newText"], "gamma");
            out.splice(start..end, b"gamma".iter().copied());
        }
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "# demo\n(gamma & beta) | !gamma\n"
        );
    }

    #[test]
    fn rename_rejects_invalid_and_constant_names() {
        let a = analysis();
        // 光标必须在变量上（列 1 的 alpha），否则在名校验之前就返回 None，
        // 测不到校验本身（变异门抓过这一点）。
        let on_alpha = Position::new(1, 1);
        assert!(
            rename(TEXT, URI, on_alpha, "1bad", &a).is_none(),
            "非法标识符"
        );
        assert!(rename(TEXT, URI, on_alpha, "true", &a).is_none(), "常量词");
        assert!(rename(TEXT, URI, on_alpha, "X", &a).is_none(), "常量词 X");
    }

    #[test]
    fn cursor_off_variable_is_none() {
        let a = analysis();
        // 第 2 行字符列 14 是 `|`。
        assert_eq!(definition(TEXT, Position::new(1, 14), &a), None);
        assert!(references(TEXT, Position::new(1, 14), true, &a).is_empty());
        assert!(highlight(TEXT, Position::new(1, 14), &a).is_empty());
        assert!(rename(TEXT, URI, Position::new(1, 14), "gamma", &a).is_none());
    }

    #[test]
    fn constant_words_are_not_variables() {
        let a = analyze("a & true");
        // 光标放在 true 上：不是变量，导航全部为空。
        assert_eq!(definition("a & true", Position::new(0, 4), &a), None);
        assert!(highlight("a & true", Position::new(0, 4), &a).is_empty());
        // 重命名到常量词名也被拒。
        assert!(rename("a & true", URI, Position::new(0, 0), "true", &a).is_none());
    }

    #[test]
    fn json_out_shapes_match_lsp() {
        let a = analysis();
        let got = definition(TEXT, Position::new(1, 1), &a).expect("定义");
        assert!(got.to_json().get("start").is_some());
        let refs = references(TEXT, Position::new(1, 1), true, &a);
        assert_eq!(refs.len(), 2);
        // json! 宏在此只做形状对照。
        let _expected_shape = json!({"uri": URI, "range": {"start": {}, "end": {}}});
    }
}
