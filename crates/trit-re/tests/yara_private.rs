//! `private` 串的集成测试（从 tests/yara_b64.rs 拆出——模块预算棘轮
//! 纪律：拆文件不抬台账）。官方语义：匹配与条件引用照常，仅报告隐藏。

use trit_core::value::Ternary;
use trit_re::sigscan::{Pattern, SigScan};
use trit_re::yaraparse::{RuleOutcome, parse_rules};

#[test]
fn private_scans_but_hides_hits() {
    // private：匹配与条件引用照常（官方语义），只是不出现在命中报告
    let src = r#"
rule u {
  strings:
    $p = "foo" private
    $b = "foo" base64
  condition:
    $p
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("两串都应可解析");
    };
    assert!(
        matches!(rule.patterns()[0].1, Pattern::Text(_)),
        "private 串照常扫描"
    );
    assert!(
        matches!(rule.patterns()[1].1, Pattern::Base64(_)),
        "base64 已是相位变体形态"
    );
    let report = SigScan::new(std::slice::from_ref(rule)).scan(b"foo", false);
    assert_eq!(
        report.rules[0].value,
        Ternary::True,
        "private 条件引用照常命中"
    );
    assert!(
        report.rules[0].hits.iter().all(|(id, _)| id != "p"),
        "private 串不出现在命中报告"
    );

    // private + 公开串共存：只隐藏 private，#p 计数照常
    let src = r#"
rule u {
  strings:
    $p = "foo" private
    $v = "foo"
  condition:
    #p == 1 and $v
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("两串都应可解析");
    };
    let report = SigScan::new(std::slice::from_ref(rule)).scan(b"foo", false);
    assert_eq!(report.rules[0].value, Ternary::True, "#p 计数照常");
    assert_eq!(
        report.rules[0].hits,
        vec![("v".to_string(), 0)],
        "只报告公开串"
    );
}
