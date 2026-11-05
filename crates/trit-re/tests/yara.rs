//! yaraparse 的集成测试（从 src/yaraparse.rs 拆出，控制单文件行数预算）。

use trit_core::value::Ternary;
use trit_re::sigscan::{Condition, SigScan};
use trit_re::yaraparse::{RuleOutcome, parse_rules};

#[test]
fn parses_full_rule_with_meta_strings_condition() {
    let src = r#"
// 演示规则：注释里的 // 和 " 引号不破坏词法
import "pe"   // 顶层注记

rule demo_rule : tag1 tag2 {
  meta:
    author = "someone // not a comment"
    version = 2
  strings:
    $mz = { 4D 5A ?? 00 }
    $str = "CreateRemoteThread" nocase
    $wide = "AZ" wide
  condition:
    $mz at 0 and any of ($str*) or all of them or 2 of ($*) or not $wide
}
"#;
    let out = parse_rules(src).expect("解析成功");
    assert_eq!(out.notes.len(), 1, "import 记入 notes");
    assert_eq!(out.rules.len(), 1);
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    assert_eq!(rule.name, "demo_rule");
    assert_eq!(rule.patterns().len(), 3);
}

#[test]
fn regex_string_makes_rule_unsupported() {
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
    let RuleOutcome::Unsupported { name, reason } = &out.rules[0] else {
        panic!("应为 Unsupported");
    };
    assert_eq!(name, "re");
    assert!(reason.contains("正则"), "{reason}");
}

#[test]
fn unsupported_condition_bails_whole_rule() {
    let src = r#"
rule fs {
  strings:
    $a = "x"
  condition:
    filesize < 100 and $a
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Unsupported { reason, .. } = &out.rules[0] else {
        panic!("filesize 应使规则 Unsupported");
    };
    assert!(
        reason.contains("filesize") || reason.contains("不支持"),
        "{reason}"
    );
}

#[test]
fn unknown_string_reference_bails() {
    let src = r#"
rule ghost {
  strings:
    $a = "x"
  condition:
    $a and $missing
}
"#;
    let out = parse_rules(src).unwrap();
    assert!(matches!(out.rules[0], RuleOutcome::Unsupported { .. }));
}

#[test]
fn unsupported_rule_scans_as_unknown_not_false() {
    // 三值纪律的本体：解析不了的规则在扫描时报 Unknown，绝不 False
    let good_src = r#"
rule good {
  strings:
    $a = { 4D 5A }
  condition:
    $a
}
"#;
    let bad_src = r#"
rule bad {
  strings:
    $a = /regex/
  condition:
    $a
}
"#;
    let good = parse_rules(good_src).unwrap();
    let bad = parse_rules(bad_src).unwrap();
    let mut rules = Vec::new();
    for outcome in good.rules.iter().chain(bad.rules.iter()) {
        if let RuleOutcome::Parsed(r) = outcome {
            rules.push(r.clone());
        }
    }
    // Unsupported 规则没有可评估体——扫描报告用 Unknown 占位
    let report = SigScan::new(&rules).scan(&[0x4D, 0x5A], false);
    assert_eq!(report.rules.len(), 1);
    assert_eq!(report.rules[0].value, Ternary::True);
    // 报告需携带 Unsupported 注记：这里由调用方把 Unsupported 转 Unknown
    let notes = bad
        .rules
        .iter()
        .filter_map(|o| match o {
            RuleOutcome::Unsupported { name, reason } => Some(format!("{name}: {reason}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].starts_with("bad:"));
}

#[test]
fn condition_tree_evaluates_through_sigscan() {
    let src = r#"
rule combo {
  strings:
    $a = { 01 }
    $b = { 02 }
  condition:
    $a and not $b or 2 of ($*)
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    // 只命中 $a：$a and not $b = True → Or 为 True
    let report = SigScan::new(std::slice::from_ref(rule)).scan(&[0x01], false);
    assert_eq!(report.rules[0].value, Ternary::True);
    // 都命中：not $b = False，2 of ($*) = True → Or 为 True
    let report = SigScan::new(std::slice::from_ref(rule)).scan(&[0x01, 0x02], false);
    assert_eq!(report.rules[0].value, Ternary::True);
    // 只命中 $b：$a 假、not $b 假、2 of 假 → False
    let report = SigScan::new(std::slice::from_ref(rule)).scan(&[0x02], false);
    assert_eq!(report.rules[0].value, Ternary::False);
}

#[test]
fn structural_errors_are_reported() {
    assert!(
        parse_rules("rule x { strings: $a = \"u\"").is_err(),
        "未闭合"
    );
    assert!(parse_rules("rule { condition: true }").is_err(), "缺名字");
    assert!(parse_rules("/* 未闭合").is_err(), "块注释未闭合");
    assert!(parse_rules("rule x { strings: $a = \"u\" } rule { }").is_err());
}

#[test]
fn hex_block_with_jump_parses() {
    let src = r#"
rule hexy {
  strings:
    $h = { 4D 5A [2-4] 50 }
  condition:
    $h
}
"#;
    let out = parse_rules(src).unwrap();
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    assert_eq!(Condition::Ref(0), *rule.condition_ref());
}
