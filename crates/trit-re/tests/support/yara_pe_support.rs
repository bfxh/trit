//! `yara_pe*` 测试共用夹具（`#[path]` 引入，不进 tests/support/mod.rs
//! 的预算台账）：最小 PE32+ 与单规则三值扫描助手。

use trit_core::value::Ternary;
use trit_re::sigscan::SigScan;
use trit_re::yaraparse::{RuleOutcome, parse_rules};

/// 最小 PE32+：machine 0x8664、2 节、timestamp 0x6000_0000、entry_rva
/// 0x1000、SizeOfHeaders 0x400、无数据目录、无 overlay、非 DLL。
pub fn tiny_pe() -> Vec<u8> {
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

/// 单规则扫描，返回三值裁决（断言规则必须可解析）。
pub fn scan_pe(src: &str, data: &[u8], pe: Option<&trit_re::pe::PeInfo>) -> Ternary {
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
