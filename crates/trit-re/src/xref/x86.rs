//! ELF32（i386）的 PLT/GOT 交叉引用。
//!
//! 与 64 位路径的三处差别：
//! - 重定位是 `SHT_REL`（8 字节/条），符号下标在 `r_info` 的高 24 位（由 `super::read_rel_entry` 处理）；
//! - PIC（PIE 是发行版默认）的 PLT 桩是 `FF A3 disp32`（`jmp *disp(%ebx)`），
//!   槽位地址 = GOT 基址 + disp；基址从 `call __x86.get_pc_thunk.bx` + `add $imm,%ebx` 解出
//!   （ebx 拿到的是 call 的返回地址，所以基址 = 返回地址 + imm）；
//! - 非 PIC 的桩是 `FF 25 abs32`，地址就是槽位。
//!
//! 解析不出 GOT 基址时**不给 `False`**：报告标成不完整，缺调用点的导入保持 `Unknown`。

use std::collections::{BTreeMap, BTreeSet};

use super::{RawSite, SiteKind, XrefReport, build_elf_report, mark_taken};
use crate::elf;

/// 各架构 ELF 扫描共用的入口形状。
pub(super) fn scan_elf(
    arch: &str,
    symbols: &[elf::DynamicSymbol],
    jump_slots: &[(u64, usize)],
    exec: &[(&elf::ElfSection, &[u8])],
    mut complete: bool,
    scanned: usize,
) -> XrefReport {
    let mut symbol_of_got: BTreeMap<u64, usize> = BTreeMap::new();
    for (got, symbol) in jump_slots {
        symbol_of_got.entry(*got).or_insert(*symbol);
    }

    let bases = find_got_bases(exec);
    if bases.is_empty() && has_pic_stubs(exec) {
        complete = false;
    }

    let mut stub_to_symbol: BTreeMap<u64, usize> = BTreeMap::new();
    let mut stub_instructions: BTreeSet<u64> = BTreeSet::new();
    let mut sites: BTreeMap<usize, BTreeSet<RawSite>> = BTreeMap::new();
    let mut taken: BTreeSet<u64> = BTreeSet::new();

    // 第一遍：经 GOT 的 jmp / call，以及只取地址的读写。
    for (section, data) in exec {
        let in_plt = section.name.starts_with(".plt");
        let mut index = 0usize;
        while index + 6 <= data.len() {
            let here = section.addr.wrapping_add(index as u64);
            let mut handled = false;
            if data[index] == 0xFF
                && let Some((slot, kind)) = got_operand(&bases, &symbol_of_got, data, index)
            {
                if kind == SiteKind::Jump && in_plt {
                    // PLT 桩只是跳板，真正调用点在调用方；这里只登记桩地址。
                    stub_to_symbol.entry(here).or_insert(symbol_of_got[&slot]);
                    stub_instructions.insert(here);
                } else {
                    sites.entry(symbol_of_got[&slot]).or_default().insert((
                        here,
                        kind,
                        section.offset as usize + index,
                    ));
                }
                handled = true;
            }
            if !handled && let Some(slot) = taken_operand(&bases, &symbol_of_got, data, index) {
                taken.insert(slot);
            }
            index += 1;
        }
    }

    // 第二遍：调用方 `E8`/`E9` 直接打到桩地址。
    for (section, data) in exec {
        let mut index = 0usize;
        while index + 5 <= data.len() {
            let here = section.addr.wrapping_add(index as u64);
            if stub_instructions.contains(&here) {
                index += 1;
                continue;
            }
            if data[index] == 0xE8 || data[index] == 0xE9 {
                let rel = i32::from_le_bytes([
                    data[index + 1],
                    data[index + 2],
                    data[index + 3],
                    data[index + 4],
                ]);
                let target = here.wrapping_add(5).wrapping_add(rel as i64 as u64);
                if let Some(symbol) = stub_to_symbol.get(&target) {
                    let kind = if data[index] == 0xE8 {
                        SiteKind::Call
                    } else {
                        SiteKind::Jump
                    };
                    sites.entry(*symbol).or_default().insert((
                        here,
                        kind,
                        section.offset as usize + index,
                    ));
                }
            }
            index += 1;
        }
    }

    let mut report = build_elf_report(arch, symbols, jump_slots, &sites, complete, scanned);
    mark_taken(&mut report.imports, &taken);
    report
}

/// `FF 25 abs32` / `FF 15 abs32`（非 PIC）与 `FF A3` / `FF 93 disp32`（PIC）解析出的槽位。
fn got_operand(
    bases: &BTreeSet<u64>,
    got: &BTreeMap<u64, usize>,
    data: &[u8],
    index: usize,
) -> Option<(u64, SiteKind)> {
    let kind = match data[index + 1] {
        0x15 | 0x93 => SiteKind::Call,
        0x25 | 0xA3 => SiteKind::Jump,
        _ => return None,
    };
    let slot = match data[index + 1] {
        0x15 | 0x25 => {
            let absolute = u32::from_le_bytes([
                data[index + 2],
                data[index + 3],
                data[index + 4],
                data[index + 5],
            ]);
            absolute as u64
        }
        _ => {
            let disp = i32::from_le_bytes([
                data[index + 2],
                data[index + 3],
                data[index + 4],
                data[index + 5],
            ]);
            resolve_ebx(bases, got, disp)?
        }
    };
    got.contains_key(&slot).then_some((slot, kind))
}

/// 只取地址（`mov`/`lea`/`push` 的 GOT 槽位操作数）：不能因此判"没有引用"。
fn taken_operand(
    bases: &BTreeSet<u64>,
    got: &BTreeMap<u64, usize>,
    data: &[u8],
    index: usize,
) -> Option<u64> {
    match (data[index], data[index + 1]) {
        // mov/lea reg, [abs32]；A1 = mov eax, moffs32；FF 35 = push [abs32]
        (0x8B | 0x8D, 0x05) | (0xA1, _) | (0xFF, 0x35) => {
            let absolute = u32::from_le_bytes([
                data[index + 2],
                data[index + 3],
                data[index + 4],
                data[index + 5],
            ]) as u64;
            got.contains_key(&absolute).then_some(absolute)
        }
        // mov/lea/push 的 EBX 相对形式。
        (0x8B | 0x8D, 0x83) | (0xFF, 0xB3) => {
            let disp = i32::from_le_bytes([
                data[index + 2],
                data[index + 3],
                data[index + 4],
                data[index + 5],
            ]);
            resolve_ebx(bases, got, disp).filter(|slot| got.contains_key(slot))
        }
        _ => None,
    }
}

/// `disp(%ebx)` → GOT 槽位。只接受落在已知槽位上的结果，基址猜错不会产生假引用。
fn resolve_ebx(bases: &BTreeSet<u64>, got: &BTreeMap<u64, usize>, disp: i32) -> Option<u64> {
    bases
        .iter()
        .map(|base| base.wrapping_add(disp as i64 as u64))
        .find(|slot| got.contains_key(slot))
}

/// 找 `call __x86.get_pc_thunk.bx; add $imm,%ebx`：返回 `ebx` 的取值（= 返回地址 + imm）。
fn find_got_bases(exec: &[(&elf::ElfSection, &[u8])]) -> BTreeSet<u64> {
    let mut bases = BTreeSet::new();
    for (section, data) in exec {
        let mut index = 0usize;
        while index + 10 <= data.len() {
            if data[index] == 0xE8 {
                let rel = i32::from_le_bytes([
                    data[index + 1],
                    data[index + 2],
                    data[index + 3],
                    data[index + 4],
                ]);
                let return_address = section.addr.wrapping_add(index as u64 + 5);
                let target = return_address.wrapping_add(rel as i64 as u64);
                if is_pc_thunk_bx(exec, target)
                    && data[index + 5] == 0x81
                    && data[index + 6] == 0xC3
                {
                    let imm = i32::from_le_bytes([
                        data[index + 7],
                        data[index + 8],
                        data[index + 9],
                        data[index + 10],
                    ]);
                    bases.insert(return_address.wrapping_add(imm as i64 as u64));
                }
            }
            index += 1;
        }
    }
    bases
}

/// `__x86.get_pc_thunk.bx` 的函数体：`mov (%esp), %ebx; ret`。
fn is_pc_thunk_bx(exec: &[(&elf::ElfSection, &[u8])], target: u64) -> bool {
    for (section, data) in exec {
        let end = section.addr.wrapping_add(data.len() as u64);
        if target < section.addr || target >= end {
            continue;
        }
        let offset = (target - section.addr) as usize;
        return data.get(offset..offset + 4) == Some(&[0x8B, 0x1C, 0x24, 0xC3][..]);
    }
    false
}

/// `.plt` 里是否存在 `FF A3`（PIC 桩）。找不到基址时用它判断"是没解析出来"而不是"没有 PIC"。
fn has_pic_stubs(exec: &[(&elf::ElfSection, &[u8])]) -> bool {
    exec.iter()
        .filter(|(section, _)| section.name.starts_with(".plt"))
        .any(|(_, data)| data.windows(2).any(|pair| pair == [0xFF, 0xA3]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::ElfSection;

    fn section(addr: u64, name: &str) -> ElfSection {
        ElfSection {
            name: name.into(),
            sh_type: 1,
            flags: 6,
            addr,
            offset: 0,
            size: 0,
            link: 0,
            entsize: 0,
            entropy: None,
        }
    }

    #[test]
    fn pc_thunk_and_add_give_the_got_base() {
        // 0x1000: call 0x1010（thunk）；0x1005: add $0x2000,%ebx ⇒ base = 0x1005 + 0x2000。
        let text = section(0x1000, ".text");
        let mut data = vec![0u8; 0x14];
        data[0] = 0xE8;
        data[1..5].copy_from_slice(&0x0Bu32.to_le_bytes());
        data[5] = 0x81;
        data[6] = 0xC3;
        data[7..11].copy_from_slice(&0x2000u32.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&[0x8B, 0x1C, 0x24, 0xC3]);
        let exec = vec![(&text, data.as_slice())];
        assert_eq!(find_got_bases(&exec), BTreeSet::from([0x3005]));
    }

    #[test]
    fn resolve_ebx_only_accepts_known_slots() {
        let bases = BTreeSet::from([0x3FCCu64]);
        let got = BTreeMap::from([(0x3FD8u64, 1usize)]);
        assert_eq!(resolve_ebx(&bases, &got, 0xC), Some(0x3FD8));
        assert_eq!(resolve_ebx(&bases, &got, 0x10), None);
        assert_eq!(resolve_ebx(&BTreeSet::new(), &got, 0xC), None);
    }

    #[test]
    fn pic_stub_without_base_marks_the_scan_incomplete() {
        let text = section(0x1000, ".text");
        let plt = section(0x2000, ".plt");
        let mut text_data = vec![0u8; 8];
        text_data[0] = 0xE8;
        text_data[1..5].copy_from_slice(&0x0u32.to_le_bytes());
        let mut plt_data = vec![0u8; 8];
        plt_data[0] = 0xFF;
        plt_data[1] = 0xA3;
        let exec = vec![(&text, text_data.as_slice()), (&plt, plt_data.as_slice())];
        let symbols = vec![elf::DynamicSymbol {
            index: 1,
            name: "fopen".into(),
            defined: false,
            kind: 2,
        }];
        let jump_slots = vec![(0x3000u64, 1usize)];
        let report = scan_elf("x86", &symbols, &jump_slots, &exec, true, 16);
        assert!(!report.complete);
        assert_eq!(report.imports.len(), 1);
        assert_eq!(report.imports[0].status, trit_core::value::Ternary::Unknown);
    }

    #[test]
    fn pic_stub_with_resolved_base_finds_the_caller() {
        // 0x1000: call thunk（0x1010）；0x1005: add $0x2000,%ebx ⇒ base 0x3005；槽位 0x3005+0xB = 0x3010。
        // 0x100B: call 0x1020（PLT 桩）；0x1020: jmp *0xB(%ebx)。
        let text = section(0x1000, ".text");
        let plt = section(0x1020, ".plt");
        let mut text_data = vec![0u8; 0x10];
        text_data[0] = 0xE8;
        text_data[1..5].copy_from_slice(&0x0Bu32.to_le_bytes());
        text_data[5] = 0x81;
        text_data[6] = 0xC3;
        text_data[7..11].copy_from_slice(&0x2000u32.to_le_bytes());
        text_data[0x0B] = 0xE8;
        text_data[0x0C..0x10].copy_from_slice(&0x10u32.to_le_bytes());
        text_data.extend_from_slice(&[0x8B, 0x1C, 0x24, 0xC3]);
        let mut plt_data = vec![0u8; 8];
        plt_data[0] = 0xFF;
        plt_data[1] = 0xA3;
        plt_data[2..6].copy_from_slice(&0xBu32.to_le_bytes());
        let exec = vec![(&text, text_data.as_slice()), (&plt, plt_data.as_slice())];
        let symbols = vec![elf::DynamicSymbol {
            index: 1,
            name: "fopen".into(),
            defined: false,
            kind: 2,
        }];
        let jump_slots = vec![(0x3010u64, 1usize)];
        let report = scan_elf("x86", &symbols, &jump_slots, &exec, true, 24);
        assert!(report.complete);
        assert_eq!(report.imports.len(), 1);
        assert_eq!(report.imports[0].status, trit_core::value::Ternary::True);
        assert_eq!(report.imports[0].sites.len(), 1);
        assert_eq!(report.imports[0].sites[0].addr, 0x100B);
    }
}
