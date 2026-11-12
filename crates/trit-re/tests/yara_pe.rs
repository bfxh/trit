//! pe.* 模块标量属性的集成测试（从 tests/yara.rs 拆出——模块预算棘轮
//! 纪律：拆文件不抬台账）。数组式 pe.sections[N] 见 yara_pe_sections.rs。

#[path = "support/yara_pe_support.rs"]
mod yara_pe_support;

use trit_core::value::Ternary;
use trit_re::yaraparse::{RuleOutcome, parse_rules};
use yara_pe_support::{scan_pe, tiny_pe};

#[test]
fn pe_scalar_properties_evaluate_with_context() {
    let data = tiny_pe();
    let info = trit_re::pe::parse(&data).expect("tiny_pe 应解析成功");
    let hit = scan_pe(
        r#"
rule pe_scalars {
  condition:
    pe.machine == 0x8664 and pe.number_of_sections == 2
    and pe.timestamp == 0x60000000 and pe.entry_point == 0x400
    and not pe.is_dll() and pe.characteristics == 0x0102
}
"#,
        &data,
        Some(&info),
    );
    assert_eq!(hit, Ternary::True, "子集标量全部命中");
    let miss = scan_pe(
        r#"
rule pe_scalars_wrong {
  condition:
    pe.number_of_sections == 3
}
"#,
        &data,
        Some(&info),
    );
    assert_eq!(miss, Ternary::False, "节数不符 = 完整证据下的判假");
}

#[test]
fn pe_boolean_functions_and_flags() {
    let data = tiny_pe();
    let info = trit_re::pe::parse(&data).expect("tiny_pe 应解析成功");
    let not_dll = scan_pe("rule b { condition: pe.is_dll() }", &data, Some(&info));
    assert_eq!(not_dll, Ternary::False, "非 DLL 样本上 is_dll() = 假");
    assert_eq!(
        scan_pe("rule b { condition: not pe.is_dll() }", &data, Some(&info)),
        Ternary::True
    );
    assert_eq!(
        scan_pe("rule b { condition: pe.is_32bit() }", &data, Some(&info)),
        Ternary::False,
        "PE32+ 上 is_32bit() = 假"
    );
    // 裸属性形态（不带 ()）：官方编译报错 wrong usage——我们整条
    // 规则 Unsupported（同型边界，不装作能评估）
    let out = parse_rules("rule b { condition: pe.is_dll }").unwrap();
    assert!(
        matches!(out.rules[0], RuleOutcome::Unsupported { .. }),
        "裸 pe.is_dll（无括号）应 Unsupported——官方 wrong usage 同型"
    );
    // DLL 位翻转 → is_dll() 变真
    let mut dll = data.clone();
    dll[0x44 + 19] |= 0x20; // Characteristics |= IMAGE_FILE_DLL (0x2000)，高字节
    let info_dll = trit_re::pe::parse(&dll).expect("DLL 位样本应解析成功");
    assert_eq!(
        scan_pe("rule b { condition: pe.is_dll() }", &dll, Some(&info_dll)),
        Ternary::True
    );
}
