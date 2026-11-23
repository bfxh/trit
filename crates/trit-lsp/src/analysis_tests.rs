//! `analysis` 的单元测试（从 analysis.rs 拆出，控制单文件行数预算）。

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
    let (markdown, range) = hover(text, Position::new(0, 2), Algebra::Kleene, &a).expect("有悬停");
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

#[test]
fn error_span_multibyte_char_boundaries() {
    use trit_core::parser::ParseError as E;
    // 2/3/4 字节字符 + 纯 ASCII 对照：跨度两端都必须是字符边界，宽度 = 命中
    // 字符的 UTF-8 字节长度。
    let cases: &[(&E, &str, usize)] = &[
        (
            &E::UnexpectedChar {
                line: 1,
                column: 3,
                ch: '¬',
            },
            "¬a¬a",
            2,
        ),
        (
            &E::UnexpectedChar {
                line: 1,
                column: 1,
                ch: '⇒',
            },
            "⇒ ≡ 0",
            3,
        ),
        (
            &E::UnexpectedChar {
                line: 1,
                column: 6,
                ch: '😀',
            },
            "emoji😀",
            4,
        ),
        (
            &E::UnexpectedChar {
                line: 1,
                column: 5,
                ch: ')',
            },
            "a & )",
            1,
        ),
    ];
    for (err, src, width) in cases {
        let (start, end) = error_span(err, src, 0);
        assert!(
            src.is_char_boundary(start),
            "start {start} 不是边界: {src:?}"
        );
        assert!(src.is_char_boundary(end), "end {end} 不是边界: {src:?}");
        assert!(start <= end && end <= src.len(), "区间越界: {src:?}");
        assert_eq!(end - start, *width, "宽度不符: {src:?}");
    }
    // 序列末尾的错误（UnexpectedEof / UnclosedParen 列位落在文末）：宽度为 0，
    // 退化为 `(start, start)` 空 span，两端仍是边界。
    for err in [
        E::UnexpectedEof { line: 1, column: 3 },
        E::UnclosedParen { line: 1, column: 3 },
    ] {
        let src = "(a";
        let (start, end) = error_span(&err, src, 0);
        assert_eq!(start, end, "文末错误应为空 span: {src:?}");
        assert!(src.is_char_boundary(start));
        assert!(start <= src.len());
    }
}

#[test]
fn multibyte_parse_error_renders_diagnostic_without_panic() {
    // 完整链路：analyze → diagnostics_json（含 `LineIndex::range` 的
    // `text[start..end]` 切片）。错误落在多字节字符上时不得触发字符边界 panic。
    for text in ["⇒ ≡ 0", "emoji😀", "¬a¬a", "(⇒"] {
        let analysis = analyze(text);
        assert!(!analysis.diagnostics.is_empty(), "{text:?} 应有错误诊断");
        let values = diagnostics_json(text, &analysis);
        assert!(!values.is_empty(), "{text:?} 应回出诊断帧");
        for value in &values {
            assert!(value["range"]["start"]["line"].is_number());
            assert!(value["range"]["end"]["line"].is_number());
        }
    }
}
