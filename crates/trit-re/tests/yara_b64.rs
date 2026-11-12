//! base64 串（相位变体）的集成测试（从 tests/yara.rs 拆出——模块预算
//! 棘轮纪律：拆文件不抬台账）。

use trit_core::value::Ternary;
use trit_re::sigscan::{Pattern, SigScan};
use trit_re::yaraparse::{RuleOutcome, parse_rules};

#[test]
fn private_is_unscannable_but_base64_scans() {
    // private：仍是 Unscannable；base64：已升级为相位变体实扫（sigscan::b64）
    let src = r#"
rule u {
  strings:
    $p = "foo" private
    $b = "foo" base64
  condition:
    $b
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("两串都应可解析");
    };
    assert!(
        matches!(rule.patterns()[0].1, Pattern::Unscannable(_)),
        "private 仍记名弃权"
    );
    assert!(
        matches!(rule.patterns()[1].1, Pattern::Base64(_)),
        "base64 已是相位变体形态"
    );
    // private 串不可评估 → 整条规则 Unknown（即使 base64 串命中）
    let report = SigScan::new(std::slice::from_ref(rule)).scan(b"Zm9v", false);
    assert_eq!(report.rules[0].value, Ternary::Unknown);
    // 纯 base64 规则：编码命中 True / 完整未命中 False
    let src = r#"
rule b {
  strings:
    $a = "foo" base64
  condition:
    $a
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("base64 规则应可解析");
    };
    let hit = SigScan::new(std::slice::from_ref(rule)).scan(b"Zm9v", false);
    assert_eq!(hit.rules[0].value, Ternary::True, "编码形态命中");
    let miss = SigScan::new(std::slice::from_ref(rule)).scan(b"foo", false);
    assert_eq!(miss.rules[0].value, Ternary::False, "明文形态不是命中");
}
