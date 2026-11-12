//! pe.* 模块属性子集的集成测试（从 tests/yara.rs 拆出——模块预算棘轮
//! 纪律：拆文件不抬台账）。

use trit_core::value::Ternary;
use trit_re::sigscan::SigScan;
use trit_re::yaraparse::{RuleOutcome, parse_rules};

/// 最小 PE32+：machine 0x8664、2 节、timestamp 0x6000_0000、entry_rva
/// 0x1000、SizeOfHeaders 0x400、无数据目录、无 overlay、非 DLL。
fn tiny_pe() -> Vec<u8> {
    let mut b = vec![0u8; 0x400];
    b[0..2].copy_from_slice(b"MZ");
    b[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    b[0x40..0x44].copy_from_slice(b"PE\0\0");
    let coff = 0x44;
    b[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
    b[coff + 2..coff + 4].copy_from_slice(&2u16.to_le_bytes());
    b[coff + 4..coff + 8].copy_from_slice(&0x6000_0000u32.to_le_bytes());
    b[coff + 16..coff + 18].copy_from_slice(&0xF0u16.to_le_bytes());
    b[coff + 18..coff + 20].copy_from_slice(&0x0102u16.to_le_bytes());
    let opt = coff + 20; // 0x58
    b[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes()); // PE32+
    b[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // entry_rva
    b[opt + 24..opt + 32].copy_from_slice(&0x400_000u64.to_le_bytes()); // image base
    b[opt + 60..opt + 64].copy_from_slice(&0x400u32.to_le_bytes()); // SizeOfHeaders
    b[opt + 68..opt + 70].copy_from_slice(&3u16.to_le_bytes()); // console
    let so = opt + 0xF0; // 节表
    let mut sect = |off: usize, name: &str, va: u32, rp: u32, chars: u32| {
        b[off..off + name.len()].copy_from_slice(name.as_bytes());
        b[off + 12..off + 16].copy_from_slice(&va.to_le_bytes());
        b[off + 16..off + 20].copy_from_slice(&0x200u32.to_le_bytes()); // raw_size
        b[off + 20..off + 24].copy_from_slice(&rp.to_le_bytes()); // raw_ptr
        b[off + 36..off + 40].copy_from_slice(&chars.to_le_bytes());
    };
    sect(so, ".text\0\0\0", 0x1000, 0x400, 0x6000_0020);
    sect(so + 40, ".rdata", 0x2000, 0x600, 0x4000_0040);
    b
}

fn scan_pe(src: &str, data: &[u8], pe: Option<&trit_re::pe::PeInfo>) -> Ternary {
    let out = parse_rules(src).expect("解析成功");
    assert_eq!(out.rules.len(), 1, "单规则");
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应为 Parsed");
    };
    SigScan::new(std::slice::from_ref(rule))
        .scan_with_context(data, false, None, pe)
        .rules[0]
        .value
}

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

#[test]
fn pe_subset_out_paths_abstain_not_guess() {
    // 子集外路径：有 PE 上下文也如实弃权（数组式 / rich 等未实现）；
    // 无 PE 上下文（ELF/裸数据）：子集内路径同样弃权，绝不判假。
    let data = tiny_pe();
    let info = trit_re::pe::parse(&data).expect("tiny_pe 应解析成功");
    for path in [
        "pe.sections[0].characteristics",
        "pe.rich_signature.version",
    ] {
        assert_eq!(
            scan_pe(
                &format!("rule u {{ condition: {path} == 1 }}"),
                &data,
                Some(&info)
            ),
            Ternary::Unknown,
            "{path} 应弃权"
        );
    }
    assert_eq!(
        scan_pe("rule u { condition: pe.machine == 0x8664 }", &data, None),
        Ternary::Unknown,
        "缺上下文 = 缺证据 = 弃权，绝不判假"
    );
}
