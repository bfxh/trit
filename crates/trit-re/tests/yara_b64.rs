//! base64 串（相位变体）的集成测试（从 tests/yara.rs 拆出——模块预算
//! 棘轮纪律：拆文件不抬台账）。private 串在 tests/yara_private.rs。

use trit_core::value::Ternary;
use trit_re::sigscan::SigScan;
use trit_re::yaraparse::{RuleOutcome, parse_rules};

#[test]
fn base64_variants_scan() {
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
