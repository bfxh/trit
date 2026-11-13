//! 单条坏规则不应拖累整份文件：官方编译错误 → 该条 `Unsupported`，
//! 后续规则仍可解析。四类旧解析器的已知整份失败写法。

use trit_re::yaraparse::{RuleOutcome, parse_rules};

fn check(bad: &str) {
    // 好规则放前面：空正则 `//` 会把本行余下内容注释掉，后面的规则天然不可见；
    // 这里验证的是"坏规则不拖累已能解析的规则 + 自己只记 Unsupported"。
    let src = format!("rule good {{ condition: true }} rule bad {{ {bad} }}");
    let out = parse_rules(&src).unwrap_or_else(|e| panic!("整份文件不应失败（{bad}）：{e}"));
    assert!(
        matches!(out.rules.first(), Some(RuleOutcome::Parsed(_))),
        "好规则应可解析：{bad}"
    );
    assert!(
        matches!(out.rules.get(1), Some(RuleOutcome::Unsupported { .. })),
        "坏规则应 Unsupported：{bad}"
    );
}

#[test]
fn regex_with_inline_flag_only_kills_its_rule() {
    check(r#"strings: $r = /x/i condition: $r"#);
}

#[test]
fn hex_alternation_only_kills_its_rule() {
    check(r#"strings: $h = { 41 | 42 } condition: $h"#);
}

#[test]
fn stray_close_paren_only_kills_its_rule() {
    check(r#"condition: 1)"#);
}

#[test]
fn empty_regex_only_kills_its_rule() {
    check(r#"strings: $r = // condition: $r"#);
}
