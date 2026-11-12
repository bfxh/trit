//! `pe.sections[N]` 数组式访问的集成测试（从 `yara_pe.rs` 拆出——模块
//! 预算棘轮纪律：拆文件不抬台账）。夹具走 `support/yara_pe_support.rs`。

#[path = "support/yara_pe_support.rs"]
mod yara_pe_support;

use trit_core::value::Ternary;
use yara_pe_support::{scan_pe, tiny_pe};

#[test]
fn pe_sections_indexed_fields_resolve() {
    let data = tiny_pe();
    let info = trit_re::pe::parse(&data).expect("tiny_pe 应解析成功");
    // 完整证据 + 字段不符 → 判假（不是弃权）
    assert_eq!(
        scan_pe(
            "rule u { condition: pe.sections[0].characteristics == 1 }",
            &data,
            Some(&info)
        ),
        Ternary::False,
        "节内确定字段 = 完整证据判假"
    );
    // 越界索引：tiny_pe 只有 2 节 → [9] = 确定的未定义 → 比较为假
    assert_eq!(
        scan_pe(
            "rule u { condition: pe.sections[9].characteristics == 1 }",
            &data,
            Some(&info)
        ),
        Ternary::False,
        "越界 = 确定的未定义（官方：比较为假）"
    );
    // 索引命中的真实值
    assert_eq!(
        scan_pe(
            "rule u { condition: pe.sections[0].raw_ptr == 0x400 }",
            &data,
            Some(&info)
        ),
        Ternary::True
    );
}

#[test]
fn pe_sections_abstain_without_context_or_member() {
    let data = tiny_pe();
    let info = trit_re::pe::parse(&data).expect("tiny_pe 应解析成功");
    // 缺 PE 上下文：sections 弃权（有索引也不知道节表）
    assert_eq!(
        scan_pe(
            "rule u { condition: pe.sections[0].characteristics == 1 }",
            &data,
            None
        ),
        Ternary::Unknown,
        "缺上下文 = 缺证据 = 弃权，绝不判假"
    );
    // name 是字符串字段（条件等值基建未建）→ 弃权
    assert_eq!(
        scan_pe(
            "rule u { condition: pe.sections[0].name == 1 }",
            &data,
            Some(&info)
        ),
        Ternary::Unknown
    );
    // 未实装路径（rich）维持弃权
    assert_eq!(
        scan_pe(
            "rule u { condition: pe.rich_signature.version == 1 }",
            &data,
            Some(&info)
        ),
        Ternary::Unknown,
        "rich 等未实装路径维持弃权"
    );
    assert_eq!(
        scan_pe("rule u { condition: pe.machine == 0x8664 }", &data, None),
        Ternary::Unknown,
        "缺上下文 = 缺证据 = 弃权，绝不判假"
    );
}
