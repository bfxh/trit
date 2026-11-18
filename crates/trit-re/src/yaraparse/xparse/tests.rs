//! 解析入口单测：条件切片、分流标志、嵌套闸（行为层面的判据在
//! `tests/yara_expr*.rs`）。

use super::*;
use crate::yaraparse::lex::lex as legacy_lex;

fn tokens(condition: &str) -> Vec<T> {
    let view = Conditions::scan(&format!("rule r {{ condition: {condition} }}"));
    assert!(view.lex_error.is_none(), "{:?}", view.lex_error);
    view.rules["r"].clone()
}

#[test]
fn scan_slices_each_rule_condition_and_collects_imports() {
    let view = Conditions::scan(
        r#"import "pe" import "math"
           private rule a : tag { meta: condition = "x" strings: $s = { 41 [2-4] 42 } condition: $s }
           rule b { condition: 1 + 1 == 2 }
           rule a { condition: false }"#,
    );
    assert_eq!(view.imports, vec![b"pe".to_vec(), b"math".to_vec()]);
    assert_eq!(view.rules.len(), 2);
    assert_eq!(
        view.rules["a"],
        vec![T::Pat("$s".into())],
        "同名规则取第一条；meta 里的 condition 不算"
    );
    assert_eq!(view.rules["b"].len(), 5);
    assert!(view.misread.is_empty());
}

#[test]
fn escaped_literals_are_flagged_for_the_new_engine() {
    let view =
        Conditions::scan(r#"rule r { condition: "\x41" == "A" } rule s { condition: "A" == "A" }"#);
    assert!(view.misread.contains("r"));
    assert!(!view.misread.contains("s"));
    assert_eq!(view.rules["r"][0], T::Str(b"A".to_vec()));
}

#[test]
fn lexical_garbage_poisons_one_token_not_the_file() {
    let view =
        Conditions::scan("rule bad { condition: 1 + \u{0}1 } rule good { condition: 1 + 1 == 2 }");
    assert!(view.lex_error.is_none());
    assert!(
        view.rules["bad"]
            .iter()
            .any(|token| matches!(token, T::Bad(_)))
    );
    assert!(
        !view.rules["good"]
            .iter()
            .any(|token| matches!(token, T::Bad(_)))
    );
}

#[test]
fn expression_markers_are_detected() {
    for condition in [
        "1 + 1 == 2",
        "filesize \\ 2 == 1",
        "filesize % 2 == 1",
        "filesize & 1 == 1",
        "filesize | 1 == 1",
        "filesize ^ 1 == 1",
        "~filesize == 1",
        "1 << 2 == 4",
        "8 >> 2 == 2",
        r#""a" contains "a""#,
        r#""a" matches /a/"#,
        "defined filesize",
        "for any i in (0..1) : (i == 0)",
        "for any i in (0..1) : (for any of them : ($))",
    ] {
        assert!(wants_expr(&tokens(condition)), "{condition}");
    }
    for condition in [
        "filesize == 13",
        "filesize - 1 == 12", // 减号留给旧解析器先看（负数字面量它会拒绝再转交）
        "$a and not $b",
        "#a > 1",
        "for any of them : ($)",
        "for all of ($a*) : (# > 2)",
        "50% of them", // 百分号紧跟 of 是量词——但旧解析器不认，走"拒绝后转交"
    ] {
        let found = wants_expr(&tokens(condition));
        assert_eq!(found, condition.starts_with("50%"), "{condition}");
    }
}

#[test]
fn depth_guard_counts_brackets_and_not_chains() {
    let guard = |condition: &str| guard_depth(&legacy_lex(condition).expect("可切词"));
    let limit = boolean::MAX_NESTING as usize;
    let wrap = |depth: usize| format!("{}true{}", "(".repeat(depth), ")".repeat(depth));
    assert!(guard(&wrap(limit)).is_ok());
    assert!(guard(&wrap(limit + 1)).is_err());
    assert!(guard(&format!("{}true", "not ".repeat(limit))).is_ok());
    assert!(guard(&format!("{}true", "not ".repeat(limit + 1))).is_err());
    // 并排的括号不叠深度；`not` 链被别的 token 打断就重新数
    assert!(guard(&vec!["(true)"; 500].join(" and ")).is_ok());
    assert!(guard(&vec!["not true"; 500].join(" and ")).is_ok());
    assert!(
        guard(&format!(
            "x{}0{}",
            "[".repeat(limit + 1),
            "]".repeat(limit + 1)
        ))
        .is_err()
    );
    // 多出来的右括号不会把计数减成负数再绕过上限
    assert!(guard(&format!("{}{}", ")".repeat(100), wrap(limit + 1))).is_err());
}
