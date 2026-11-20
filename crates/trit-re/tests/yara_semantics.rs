//! YARA 文本语义边界（wide 非 ASCII / 重叠计数 / 条件递归深度）。
//!
//! 从 `yara.rs` 拆出，守单文件行数预算；判定口径与官方 yara-python 4.5.4 对拍。

use trit_core::value::Ternary;
use trit_re::sigscan::SigScan;
use trit_re::yaraparse::{RuleOutcome, parse_rules};

fn scan_one(src: &str, data: &[u8]) -> Ternary {
    let out = parse_rules(src).unwrap();
    assert_eq!(out.rules.len(), 1, "单规则文件");
    match &out.rules[0] {
        RuleOutcome::Parsed(rule) => {
            SigScan::new(std::slice::from_ref(rule))
                .scan(data, false)
                .rules[0]
                .value
        }
        RuleOutcome::Unsupported { name, reason } => {
            panic!("规则 {name} 意外 Unsupported：{reason}");
        }
    }
}

#[test]
fn overlapping_text_hits_are_counted() {
    // 官方 YARA 对 `aaa` 里的 `"aa"` 计 2 个命中（起点 0、1）。
    let src = r#"
rule overlap {
  strings:
    $a = "aa"
  condition:
    #a == 2 and @a[2] == 1
}
"#;
    assert_eq!(scan_one(src, b"aaa"), Ternary::True);
}

#[test]
fn wide_non_ascii_uses_utf8_bytes_not_utf16le() {
    // 官方 yara-python 4.5.4：`"é" wide` 展开为 UTF-8 字节逐个补零
    // `c3 00 a9 00`；UTF-16LE 的 `e9 00` 不应命中。
    let src = r#"
rule wide_non_ascii {
  strings:
    $w = "é" wide
  condition:
    $w
}
"#;
    assert_eq!(scan_one(src, &[0xc3, 0x00, 0xa9, 0x00]), Ternary::True);
    assert_eq!(scan_one(src, &[0xe9, 0x00]), Ternary::False);
}

#[test]
fn nested_condition_depth_is_bounded() {
    // 修复前 release 8,000 层括号栈溢出；这里用 600 层确保 debug 也不会压垮测试进程。
    let depth = 600usize;
    let mut src = String::from("rule deep { condition: ");
    src.push_str(&"(".repeat(depth));
    src.push_str("true");
    src.push_str(&")".repeat(depth));
    src.push_str(" }\n");
    let out = parse_rules(&src).expect("结构级解析应成功");
    let RuleOutcome::Unsupported { reason, .. } = &out.rules[0] else {
        panic!("超过嵌套上限应如实 Unsupported");
    };
    assert!(reason.contains("嵌套超过上限"), "{reason}");
}
