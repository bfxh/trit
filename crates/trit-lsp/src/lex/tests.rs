use super::*;

fn kinds(text: &str) -> Vec<(Kind, String)> {
    tokens(text).into_iter().map(|t| (t.kind, t.text)).collect()
}

/// The lexer indexes `text[i..]` and `next_char` expects `i` on a char boundary.
/// That is true today only because every advance uses `len_utf8()`; one future
/// `i += 1` on a non-ASCII branch turns it into a panic on user text. This test makes
/// the property explicit: every token must start and end on a char boundary, and the
/// byte range must reproduce the token text exactly. Slicing here panics if that is
/// ever violated, so the test fails on the same input a user would crash on.
#[test]
fn every_token_lands_on_char_boundaries() {
    let samples = [
        "",
        " ",
        "\n\n",
        "a",
        "()",
        "a<=>b",
        "¬a",
        "a ⊕ b",
        "a ∨ b",
        "a ∧ ¬b",
        "中文",
        "变量 与 常量",
        "a 中 b",
        "日本語のテキスト",
        "🚀",
        "🚀🚀",
        "a🚀b",
        "e\u{0301}",
        "e\u{0301}a",
        "a\u{200b}b",
        "# 中文注释\na",
        "# 🚀\na & b",
        "\u{feff}a",
        "a\r\nb",
        "???",
        "0 1 ?",
        "X x Unknown unknown",
        "a <=> b => c && d || e == f",
        "¬¬¬",
        "→⇒≡",
        "a\u{0301}\u{0301}\u{0301}b",
        "混合 text with 中文 and 🚀 and ¬",
    ];
    for s in samples {
        let toks = tokens(s);
        for t in &toks {
            assert!(t.start <= t.end, "start > end in {s:?}");
            assert!(t.end <= s.len(), "end past the string in {s:?}");
            assert!(
                s.is_char_boundary(t.start),
                "start not on a boundary in {s:?}"
            );
            assert!(s.is_char_boundary(t.end), "end not on a boundary in {s:?}");
            assert_eq!(&s[t.start..t.end], t.text, "token text mismatch in {s:?}");
        }
        for w in toks.windows(2) {
            assert!(w[0].end <= w[1].start, "overlapping tokens in {s:?}");
        }
    }
}

#[test]
fn splits_expression() {
    let got = kinds("(a & b) | !c");
    let got: Vec<(Kind, &str)> = got.iter().map(|(k, s)| (*k, s.as_str())).collect();
    assert_eq!(
        got,
        vec![
            (Kind::LParen, "("),
            (Kind::Ident, "a"),
            (Kind::Operator, "&"),
            (Kind::Ident, "b"),
            (Kind::RParen, ")"),
            (Kind::Operator, "|"),
            (Kind::Operator, "!"),
            (Kind::Ident, "c"),
        ]
    );
}

#[test]
fn longest_match_wins() {
    let got = kinds("a <=> b => c && d || e == f");
    let ops: Vec<&str> = got
        .iter()
        .filter(|(k, _)| *k == Kind::Operator)
        .map(|(_, s)| s.as_str())
        .collect();
    assert_eq!(ops, vec!["<=>", "=>", "&&", "||", "=="]);
}

#[test]
fn comments_and_constants() {
    let got = kinds("# 注释\n0 & 1 | X && true");
    let constants: Vec<&str> = got
        .iter()
        .filter(|(k, _)| *k == Kind::Constant)
        .map(|(_, s)| s.as_str())
        .collect();
    assert_eq!(constants, vec!["0", "1", "X", "true"]);
}

#[test]
fn unicode_operator_offsets_are_bytes() {
    let text = "a ⊕ b";
    let op = tokens(text)
        .into_iter()
        .find(|t| t.kind == Kind::Operator)
        .expect("有运算符");
    assert_eq!(&text[op.start..op.end], "⊕");
    assert_eq!(op.end - op.start, 3);
}

#[test]
fn operators_round_trip() {
    // 每个拼写都必须真能被解析器接受，且样例能求值——防止 LSP 和语言分叉。
    for op in OPS {
        for spelling in op.spellings {
            assert_eq!(
                operator(spelling).map(|o| o.name),
                Some(op.name),
                "{spelling} 查不到"
            );
        }
        trit_core::Parser::parse(op.sample)
            .unwrap_or_else(|e| panic!("样例 {:?} 解析失败：{e}", op.sample));
    }
}
