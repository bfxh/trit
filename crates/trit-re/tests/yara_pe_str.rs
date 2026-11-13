//! `pe.sections[N].name` 字符串比较 + `pe.rich_signature.*` 的属性测试。

#[path = "support/yara_pe_support.rs"]
mod yara_pe_support;

use trit_core::value::Ternary;
use yara_pe_support::{scan_pe, tiny_pe};

/// 带 Rich header 的最小 PE32+（PE header 后移到 0x80，0x40 起放
/// DanS/entry/Rich）。
fn rich_pe() -> Vec<u8> {
    let base = tiny_pe();
    let mut b = vec![0u8; 0x400];
    b[..0x40].copy_from_slice(&base[..0x40]);
    b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes()); // e_lfanew
    b[0x80..0x200].copy_from_slice(&base[0x40..0x1c0]); // PE header + 节表
    let key = 0x1234_5678u32;
    let enc = |v: u32| (v ^ key).to_le_bytes();
    b[0x40..0x44].copy_from_slice(&enc(0x536e_6144)); // DanS
    b[0x44..0x48].copy_from_slice(&enc(0));
    b[0x48..0x4c].copy_from_slice(&enc(0));
    b[0x4c..0x50].copy_from_slice(&enc(0));
    b[0x50..0x54].copy_from_slice(&enc((0x93u32 << 16) | 0x7809)); // comp_id
    b[0x54..0x58].copy_from_slice(&enc(1)); // count
    b[0x58..0x5c].copy_from_slice(b"Rich");
    b[0x5c..0x60].copy_from_slice(&key.to_le_bytes());
    b
}

#[test]
fn section_name_equality_is_three_valued() {
    let data = tiny_pe();
    let info = trit_re::pe::parse(&data).expect("tiny_pe 应解析成功");
    assert_eq!(
        scan_pe(
            "rule s { condition: pe.sections[0].name == \".text\" }",
            &data,
            Some(&info)
        ),
        Ternary::True
    );
    assert_eq!(
        scan_pe(
            "rule s { condition: pe.sections[0].name != \".data\" }",
            &data,
            Some(&info)
        ),
        Ternary::True
    );
    // 越界索引 = 确定的未定义 → 比较为假
    assert_eq!(
        scan_pe(
            "rule s { condition: pe.sections[9].name == \".text\" }",
            &data,
            Some(&info)
        ),
        Ternary::False
    );
    // 缺 PE 上下文 = 弃权
    assert_eq!(
        scan_pe(
            "rule s { condition: pe.sections[0].name == \".text\" }",
            &data,
            None
        ),
        Ternary::Unknown
    );
    // 非字符串属性与字符串比较 = 解析层拒绝（整条 Unsupported）
    let out = trit_re::yaraparse::parse_rules("rule s { condition: pe.machine == \"x\" }").unwrap();
    assert!(matches!(
        out.rules[0],
        trit_re::yaraparse::RuleOutcome::Unsupported { .. }
    ));
}

#[test]
fn rich_signature_numeric_and_string_props() {
    let data = rich_pe();
    let info = trit_re::pe::parse(&data).expect("rich_pe 应解析成功");
    assert_eq!(
        scan_pe(
            "rule r { condition: pe.rich_signature.version == 0x93 and pe.rich_signature.key == 0x12345678 }",
            &data,
            Some(&info)
        ),
        Ternary::True
    );
    assert_eq!(
        scan_pe(
            "rule r { condition: pe.rich_signature.offset == 0x40 and pe.rich_signature.length == 24 }",
            &data,
            Some(&info)
        ),
        Ternary::True
    );
    // clear_data 含二进制字节：只验证比较语义（不等于随机串）
    assert_eq!(
        scan_pe(
            "rule r { condition: pe.rich_signature.clear_data != \"x\" }",
            &data,
            Some(&info)
        ),
        Ternary::True
    );
    // 无 Rich header = 确定的未定义 → 比较为假
    let plain = tiny_pe();
    let plain_info = trit_re::pe::parse(&plain).expect("tiny_pe 应解析成功");
    assert_eq!(
        scan_pe(
            "rule r { condition: pe.rich_signature.version == 1 }",
            &plain,
            Some(&plain_info)
        ),
        Ternary::False
    );
    // 缺 PE 上下文 = 弃权
    assert_eq!(
        scan_pe(
            "rule r { condition: pe.rich_signature.key == 1 }",
            &data,
            None
        ),
        Ternary::Unknown
    );
}
