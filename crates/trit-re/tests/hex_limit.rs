//! hex 段数上限的集成测试（从 yara.rs 拆出，控制单文件行数预算）。

use trit_re::yaraparse::{RuleOutcome, parse_rules};

#[test]
fn overlong_hex_rule_is_unsupported_not_abort() {
    // 段数超上限：整条规则记 Unsupported（结构化拒绝），不 abort、不假命中；
    // 上限内的同形规则照常 Parsed。
    let mut over = String::from("4D");
    for _ in 0..4096 {
        over.push_str(" [0] 01"); // 4097 段 > 4096 上限
    }
    let src = format!("rule big {{ strings: $a = {{ {over} }} condition: $a }}");
    let out = parse_rules(&src).expect("规则文件可解析");
    assert!(
        matches!(out.rules[0], RuleOutcome::Unsupported { .. }),
        "超限 hex 规则应 Unsupported"
    );

    let mut ok = String::from("4D");
    for _ in 0..4095 {
        ok.push_str(" [0] 01"); // 4096 段 = 上限
    }
    let src = format!("rule fine {{ strings: $a = {{ {ok} }} condition: $a }}");
    let out = parse_rules(&src).expect("规则文件可解析");
    assert!(
        matches!(out.rules[0], RuleOutcome::Parsed(_)),
        "上限内应 Parsed"
    );
}
