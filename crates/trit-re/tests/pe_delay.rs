//! PE delay-load 导入回归（拆出主 PE 测试守行数预算）。

mod support;

use trit_core::value::Ternary;
use trit_re::{analyze, pe};

#[test]
fn parses_delay_load_imports_without_false_negative() {
    // 在合成 PE32+ 的 .text 空白处塞一个 delay-load 描述符：
    // DELAY.dll!VirtualAllocEx（RVA 形态，Attributes=1）。
    let mut bytes = support::pe_sample();
    support::put_u32(&mut bytes, 0x170, 0x1030); // data dir 13: delay import
    support::put_u32(&mut bytes, 0x174, 0x40);
    support::put_u32(&mut bytes, 0x230, 1); // Attributes = dlattrRva
    support::put_u32(&mut bytes, 0x234, 0x1070); // DllNameRVA
    support::put_u32(&mut bytes, 0x238, 0);
    support::put_u32(&mut bytes, 0x23C, 0x1098); // IAT
    support::put_u32(&mut bytes, 0x240, 0x1080); // INT
    support::put_u32(&mut bytes, 0x244, 0);
    support::put_u32(&mut bytes, 0x248, 0);
    support::put_u32(&mut bytes, 0x24C, 0);
    bytes[0x250..0x270].fill(0); // 结束描述符
    bytes[0x270..0x27A].copy_from_slice(b"DELAY.dll\0");
    support::put_u64(&mut bytes, 0x280, 0x10B0); // INT[0] -> hint/name
    support::put_u64(&mut bytes, 0x288, 0); // INT 结束
    bytes[0x298..0x2A8].fill(0); // IAT 内容延迟加载前可为 0
    support::put_u16(&mut bytes, 0x2B0, 0);
    bytes[0x2B2..0x2C1].copy_from_slice(b"VirtualAllocEx\0");

    let info = pe::parse(&bytes).expect("delay PE must parse");
    let dlls = info.imports.as_ref().expect("imports parsed");
    assert_eq!(dlls.len(), 2, "标准导入 + delay 导入都要在表里");
    let delay = dlls
        .iter()
        .find(|d| d.name == "DELAY.dll")
        .expect("delay DLL");
    assert_eq!(delay.functions.len(), 1);
    assert_eq!(delay.functions[0].name, "VirtualAllocEx");
    assert_eq!(delay.functions[0].slot_rva, Some(0x1098));

    let triage = analyze(&bytes);
    assert_eq!(
        triage.facts.get("imports.inj.alloc"),
        Some(Ternary::True),
        "delay 表里的 VirtualAllocEx 不能被漏成 Unknown/False"
    );
}
