//! 正则串（Pike VM）的集成测试（从 tests/yara.rs 拆出，控制单文件行数预算）。

use trit_core::value::Ternary;
use trit_re::sigscan::{Pattern, SigScan};
use trit_re::yaraparse::{RuleOutcome, parse_rules};

#[test]
fn regex_string_parses_and_scans() {
    // 正则串走 Pike VM（sigscan::regex）：命中 True / 完整未命中 False
    let src = r#"
rule re {
  strings:
    $a = /mshta\.exe/ nocase
  condition:
    $a
}
"#;
    let out = parse_rules(src).unwrap();
    assert_eq!(out.rules.len(), 1);
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("正则串应解析为可扫描形态");
    };
    assert!(
        matches!(rule.patterns()[0].1, Pattern::Regex(_)),
        "串应编译为 Pattern::Regex"
    );
    let report = SigScan::new(std::slice::from_ref(rule)).scan(b"mshta.exe", false);
    assert_eq!(report.rules[0].value, Ternary::True, "nocase 命中");
    let miss = SigScan::new(std::slice::from_ref(rule)).scan(b"mshta.dll", false);
    assert_eq!(
        miss.rules[0].value,
        Ternary::False,
        "完整扫描未命中 = False"
    );
}
