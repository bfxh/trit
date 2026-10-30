//! `xref` 的单元与集成式单元测试（从 mod.rs 拆出，控制单文件行数预算）。

use super::{ImportXref, SiteKind, build_elf_report, mark_taken, scan_pe};
use crate::elf::DynamicSymbol;
use crate::pe;
use std::collections::{BTreeMap, BTreeSet};
use trit_core::value::Ternary;

fn tiny_pe(call_disp: i32) -> Vec<u8> {
    // 最小 PE32+：.text（可执行）+ .rdata（IAT），各 0x200 字节。
    let mut file = vec![0u8; 0x600];
    file[..2].copy_from_slice(b"MZ");
    file[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    file[0x40..0x44].copy_from_slice(b"PE\0\0");
    let coff = 0x44;
    file[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
    file[coff + 2..coff + 4].copy_from_slice(&2u16.to_le_bytes());
    file[coff + 16..coff + 18].copy_from_slice(&0xF0u16.to_le_bytes());
    let opt = coff + 20;
    file[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
    file[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes());
    file[opt + 24..opt + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
    file[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes());
    file[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
    // 数据目录 1 = 导入表（目录 0 是导出，这里留零）。
    file[opt + 120..opt + 124].copy_from_slice(&0x2000u32.to_le_bytes());
    file[opt + 124..opt + 128].copy_from_slice(&40u32.to_le_bytes());
    let sections = opt + 0xF0;
    file[sections..sections + 8].copy_from_slice(b".text\0\0\0");
    file[sections + 8..sections + 12].copy_from_slice(&0x200u32.to_le_bytes());
    file[sections + 12..sections + 16].copy_from_slice(&0x1000u32.to_le_bytes());
    file[sections + 16..sections + 20].copy_from_slice(&0x200u32.to_le_bytes());
    file[sections + 20..sections + 24].copy_from_slice(&0x200u32.to_le_bytes());
    file[sections + 36..sections + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes());
    let second = sections + 40;
    file[second..second + 8].copy_from_slice(b".rdata\0\0");
    file[second + 8..second + 12].copy_from_slice(&0x200u32.to_le_bytes());
    file[second + 12..second + 16].copy_from_slice(&0x2000u32.to_le_bytes());
    file[second + 16..second + 20].copy_from_slice(&0x200u32.to_le_bytes());
    file[second + 20..second + 24].copy_from_slice(&0x400u32.to_le_bytes());
    file[second + 36..second + 40].copy_from_slice(&0x4000_0040u32.to_le_bytes());

    let descriptor = 0x400usize;
    file[descriptor..descriptor + 4].copy_from_slice(&0x2040u32.to_le_bytes());
    file[descriptor + 12..descriptor + 16].copy_from_slice(&0x2080u32.to_le_bytes());
    file[descriptor + 16..descriptor + 20].copy_from_slice(&0x2040u32.to_le_bytes());
    file[0x440..0x448].copy_from_slice(&0x2060u64.to_le_bytes());
    // 0x2060 → 文件 0x460；0x2080 → 文件 0x480。
    file[0x460..0x462].copy_from_slice(&0u16.to_le_bytes());
    let load_library = b"LoadLibraryA\0";
    file[0x462..0x462 + load_library.len()].copy_from_slice(load_library);
    let kernel32 = b"KERNEL32.dll\0";
    file[0x480..0x480 + kernel32.len()].copy_from_slice(kernel32);

    let site = 0x210usize;
    file[site] = 0xFF;
    file[site + 1] = 0x15;
    file[site + 2..site + 6].copy_from_slice(&call_disp.to_le_bytes());
    file
}

#[test]
fn finds_call_through_iat() {
    // 槽位 VA = base + 0x2040；下一条指令 RVA = 0x1016；disp = 0x2040 - 0x1016。
    let file = tiny_pe(0x102a);
    let info = pe::parse(&file).expect("parse");
    let report = scan_pe(&file, &info);
    assert!(report.complete);
    let row = &report.imports[0];
    assert_eq!(row.import, "KERNEL32.dll!LoadLibraryA");
    assert_eq!(row.status, Ternary::True);
    assert_eq!(row.sites.len(), 1);
    assert_eq!(row.sites[0].addr, 0x1010);
    assert_eq!(row.sites[0].file_offset, 0x210);
    assert_eq!(row.sites[0].kind, SiteKind::Call);
}

#[test]
fn follows_import_thunk_to_caller() {
    // 0x1010: call 0x1100（跳板）；0x1100: jmp [IAT 0x2040]
    let mut file = tiny_pe(0);
    file[0x210] = 0xE8;
    file[0x211..0x215].copy_from_slice(&0xEBu32.to_le_bytes());
    let thunk = 0x300usize;
    file[thunk] = 0xFF;
    file[thunk + 1] = 0x25;
    file[thunk + 2..thunk + 6].copy_from_slice(&0xF3Au32.to_le_bytes());
    let info = pe::parse(&file).expect("parse");
    let report = scan_pe(&file, &info);
    let row = &report.imports[0];
    assert_eq!(row.status, Ternary::True);
    assert_eq!(row.sites.len(), 1);
    assert_eq!(row.sites[0].addr, 0x1010);
    assert_eq!(row.sites[0].kind, SiteKind::Call);
}

#[test]
fn wrong_target_does_not_match() {
    let file = tiny_pe(0x102b);
    let info = pe::parse(&file).expect("parse");
    let report = scan_pe(&file, &info);
    assert_eq!(report.imports[0].status, Ternary::False);
    assert!(report.imports[0].sites.is_empty());
}

#[test]
fn unsupported_arch_stays_unknown() {
    let mut file = tiny_pe(0x102a);
    // ARM32 目前不在 xref 支持面内（ARM64 已支持）。
    file[0x44..0x46].copy_from_slice(&0x01c0u16.to_le_bytes());
    let info = pe::parse(&file).expect("parse");
    let report = scan_pe(&file, &info);
    assert!(!report.complete);
    assert_eq!(report.imports[0].status, Ternary::Unknown);
}

#[test]
fn defined_symbols_with_jump_slots_are_not_imports() {
    let symbols = vec![
        DynamicSymbol {
            index: 1,
            name: "malloc".into(),
            defined: true,
            kind: 2,
        },
        DynamicSymbol {
            index: 2,
            name: "__tls_get_addr".into(),
            defined: false,
            kind: 2,
        },
    ];
    let slots = vec![(0x2000u64, 1usize), (0x2008, 2)];
    let report = build_elf_report("elf64", &symbols, &slots, &BTreeMap::new(), true, 0);
    assert_eq!(report.imports.len(), 1);
    assert_eq!(report.imports[0].import, "__tls_get_addr");
}

#[test]
fn taken_slot_downgrades_false_to_unknown() {
    let mut rows = vec![ImportXref {
        import: "a!b".into(),
        slot: 0x10,
        sites: Vec::new(),
        status: Ternary::False,
        note: String::new(),
    }];
    let taken: BTreeSet<u64> = [0x10u64].into();
    mark_taken(&mut rows, &taken);
    assert_eq!(rows[0].status, Ternary::Unknown);
    assert!(rows[0].note.contains("地址"));
}

#[test]
fn x64_address_only_import_is_unknown() {
    let mut file = tiny_pe(0);
    // 把 0x210 的 FF15 换成 48 8D 05 disp32（lea rax, [rip+slot]）。
    file[0x210..0x217].copy_from_slice(&[0x48, 0x8D, 0x05, 0, 0, 0, 0]);
    let disp = 0x1029i32; // slot 0x2040 - 下一条指令 0x1017
    file[0x213..0x217].copy_from_slice(&disp.to_le_bytes());
    let info = pe::parse(&file).expect("parse");
    let report = scan_pe(&file, &info);
    assert!(report.imports[0].sites.is_empty());
    assert_eq!(report.imports[0].status, Ternary::Unknown);
    assert!(report.imports[0].note.contains("地址"));
}

#[test]
fn elf_report_skips_object_symbols_without_slots() {
    let symbols = vec![
        DynamicSymbol {
            index: 1,
            name: "socket".into(),
            defined: false,
            kind: 2,
        },
        DynamicSymbol {
            index: 2,
            name: "environ".into(),
            defined: false,
            kind: 1,
        },
    ];
    let report = build_elf_report("elf64", &symbols, &[], &BTreeMap::new(), true, 0);
    assert_eq!(report.imports.len(), 1);
    assert_eq!(report.imports[0].import, "socket");
    // 没有 JUMP_SLOT 槽位 ⇒ 不能判"没有调用点"。
    assert_eq!(report.imports[0].status, Ternary::Unknown);
}
