//! ELF32（i386）的 PLT/GOT 交叉引用。
//!
//! 与 64 位路径的差别：
//! - 重定位是 `SHT_REL`（8 字节/条），符号下标在 `r_info` 的高 24 位（由 `super::read_rel_entry` 处理）；
//! - PIC（PIE 是发行版默认）的 PLT 桩是 `FF A3 disp32`（`jmp *disp(%ebx)`），
//!   槽位地址 = GOT 基址 + disp；基址从 `call __x86.get_pc_thunk.<reg>` + `add $imm,<reg>` 解出
//!   （寄存器拿到的是 call 的返回地址，所以基址 = 返回地址 + imm）。GCC 会用
//!   `get_pc_thunk.{ax,bx,cx,dx,si,di}` 中的任意一个，所以**按寄存器分别跟踪**；
//! - 非 PIC 的桩是 `FF 25 abs32`，地址就是槽位。
//!
//! 解析不出 GOT 基址时**不给 `False`**：报告标成不完整，缺调用点的导入保持 `Unknown`。

use std::collections::{BTreeMap, BTreeSet};

use super::{RawSite, SiteKind, XrefReport, build_elf_report, mark_taken};
use crate::elf;

/// GOT 基址：寄存器号（0=eax … 7=edi）→ 可能的基址值。
///
/// 同一个二进制里可能同时存在 `get_pc_thunk.ax` 与 `.bx`（不同函数各取所需），
/// 所以每个寄存器各自一堆候选；判定时只接受"基址 + 位移落在已知槽位"的结果。
type Bases = BTreeMap<u8, BTreeSet<u64>>;

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
                let Some(rel) = read_i32(data, index + 1) else {
                    index += 1;
                    continue;
                };
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
    mark_immediate_addresses(exec, &stub_to_symbol, &symbol_of_got, &mut taken);
    mark_taken(&mut report.imports, &taken);
    report
}

/// 第三遍：立即数取址。
///
/// 非 PIC 下取导入函数地址拿到的是 **PLT 桩地址**（不是 GOT 槽位）：
/// `push $printf@plt`（`68 imm32`）、`mov r32, imm32`（`B8..BF`）、
/// `mov [ebp+d8], imm32`（`C7 45`）、`mov [ebp+d32], imm32`（`C7 85`）、
/// `mov [abs32], imm32`（`C7 05`）。立即数等于已知桩地址或 GOT 槽位才采信，
/// 只把对应行降级为 `Unknown`，不产生调用点。
fn mark_immediate_addresses(
    exec: &[(&elf::ElfSection, &[u8])],
    stubs: &BTreeMap<u64, usize>,
    symbol_of_got: &BTreeMap<u64, usize>,
    taken: &mut BTreeSet<u64>,
) {
    let mut slot_of_symbol: BTreeMap<usize, u64> = BTreeMap::new();
    for (slot, symbol) in symbol_of_got {
        slot_of_symbol.entry(*symbol).or_insert(*slot);
    }
    for (_, data) in exec {
        let mut index = 0usize;
        while index + 5 <= data.len() {
            let Some(immediate) = immediate_reference(data, index) else {
                index += 1;
                continue;
            };
            let value = immediate as u64;
            if symbol_of_got.contains_key(&value) {
                taken.insert(value);
            } else if let Some(symbol) = stubs.get(&value)
                && let Some(slot) = slot_of_symbol.get(symbol)
            {
                taken.insert(*slot);
            }
            index += 1;
        }
    }
}

/// 取一条指令里可能被当作地址的 32 位立即数（只认已列出的编码）。
fn immediate_reference(data: &[u8], index: usize) -> Option<u32> {
    let at = match data[index] {
        0x68 | 0xB8..=0xBF => index + 1,
        0xC7 => match data.get(index + 1)? {
            // C7 /0：modrm + 寻址之后才是 imm32。
            0x45 => index + 3, // [ebp+disp8]
            0x85 => index + 6, // [ebp+disp32]
            0x05 => index + 6, // [abs32]
            _ => return None,
        },
        _ => return None,
    };
    read_u32(data, at)
}

/// `FF /2`、`FF /4` 经 GOT 的调用点；基址寄存器不限（eax/ebx/…）。
fn got_operand(
    bases: &Bases,
    got: &BTreeMap<u64, usize>,
    data: &[u8],
    index: usize,
) -> Option<(u64, SiteKind)> {
    let modrm = data[index + 1];
    let (slot, kind) = match modrm {
        // FF /2 abs32 = call *abs32；FF /4 abs32 = jmp *abs32
        0x15 => (read_u32(data, index + 2)? as u64, SiteKind::Call),
        0x25 => (read_u32(data, index + 2)? as u64, SiteKind::Jump),
        _ => {
            let register = disp32_base(modrm)?;
            let kind = match modrm & 0xF8 {
                0x90 => SiteKind::Call,
                0xA0 => SiteKind::Jump,
                _ => return None,
            };
            let disp = read_i32(data, index + 2)?;
            (resolve_base(bases, got, register, disp)?, kind)
        }
    };
    got.contains_key(&slot).then_some((slot, kind))
}

/// 只取地址（`mov`/`lea`/`cmp`/`test`/`push` 的 GOT 槽位操作数）：不能因此判"没有引用"。
fn taken_operand(
    bases: &Bases,
    got: &BTreeMap<u64, usize>,
    data: &[u8],
    index: usize,
) -> Option<u64> {
    let second = data[index + 1];
    match data[index] {
        // mov/lea/cmp/test reg, [abs32] / [reg+disp32]
        0x8B | 0x8D | 0x39 | 0x3B | 0x85 => {
            if second == 0x05 {
                return absolute_slot(got, data, index + 2);
            }
            relative_slot(bases, got, disp32_base(second)?, data, index + 2)
        }
        // cmp r/m8|32, imm8|32：绝对 `3D` 或 `mod=10, rm=reg`（立即数不参与判定）。
        0x80 | 0x81 | 0x83 => {
            if second == 0x3D {
                return absolute_slot(got, data, index + 2);
            }
            relative_slot(bases, got, disp32_base(second)?, data, index + 2)
        }
        // A1 = `mov eax, moffs32`：绝对寻址。
        0xA1 => absolute_slot(got, data, index + 1),
        // FF /6 = push [abs32] / push [reg+disp32]
        0xFF => {
            if second == 0x35 {
                return absolute_slot(got, data, index + 2);
            }
            if second & 0xF8 == 0xB0 {
                return relative_slot(bases, got, disp32_base(second)?, data, index + 2);
            }
            None
        }
        // mov r32, imm32（B8..BF）：立即数恰好等于槽位地址 ⇒ 也是取址。
        0xB8..=0xBF => {
            let immediate = read_u32(data, index + 1)? as u64;
            got.contains_key(&immediate).then_some(immediate)
        }
        _ => None,
    }
}

fn absolute_slot(got: &BTreeMap<u64, usize>, data: &[u8], at: usize) -> Option<u64> {
    let address = read_u32(data, at)? as u64;
    got.contains_key(&address).then_some(address)
}

fn relative_slot(
    bases: &Bases,
    got: &BTreeMap<u64, usize>,
    register: u8,
    data: &[u8],
    at: usize,
) -> Option<u64> {
    resolve_base(bases, got, register, read_i32(data, at)?)
}

/// `disp32(%reg)` → GOT 槽位。只接受落在已知槽位上的结果，基址猜错不会产生假引用。
fn resolve_base(bases: &Bases, got: &BTreeMap<u64, usize>, register: u8, disp: i32) -> Option<u64> {
    bases
        .get(&register)?
        .iter()
        .map(|base| base.wrapping_add(disp as i64 as u64))
        .find(|slot| got.contains_key(slot))
}

/// `mod=10`（disp32）且 `rm` 不是 SIB 时的基址寄存器。
fn disp32_base(modrm: u8) -> Option<u8> {
    if modrm & 0xC0 != 0x80 {
        return None;
    }
    let register = modrm & 7;
    (register != 4).then_some(register)
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_i32(data: &[u8], at: usize) -> Option<i32> {
    read_u32(data, at).map(|value| value as i32)
}

/// 找 `call __x86.get_pc_thunk.<reg>; add $imm,<reg>`，按寄存器收集候选基址。
fn find_got_bases(exec: &[(&elf::ElfSection, &[u8])]) -> Bases {
    let mut bases: Bases = BTreeMap::new();
    for (section, data) in exec {
        let mut index = 0usize;
        while index + 5 <= data.len() {
            if data[index] != 0xE8 {
                index += 1;
                continue;
            }
            let Some(rel) = read_i32(data, index + 1) else {
                index += 1;
                continue;
            };
            let return_address = section.addr.wrapping_add(index as u64 + 5);
            let target = return_address.wrapping_add(rel as i64 as u64);
            let Some(register) = pc_thunk_register(exec, target) else {
                index += 1;
                continue;
            };
            if let Some(imm) = add_immediate(data, index + 5, register) {
                bases
                    .entry(register)
                    .or_default()
                    .insert(return_address.wrapping_add(imm as i64 as u64));
            }
            index += 1;
        }
    }
    bases
}

/// `add $imm, %reg`：eax 有短编码 `05 imm32`，其余是 `81 C0|reg imm32`。
fn add_immediate(data: &[u8], at: usize, register: u8) -> Option<u32> {
    if register == 0 && data.get(at) == Some(&0x05) {
        return read_u32(data, at + 1);
    }
    if data.get(at) == Some(&0x81) && data.get(at + 1) == Some(&(0xC0 | register)) {
        return read_u32(data, at + 2);
    }
    None
}

/// `__x86.get_pc_thunk.<reg>` 的函数体：`mov (%esp), %reg; ret` → 寄存器号。
fn pc_thunk_register(exec: &[(&elf::ElfSection, &[u8])], target: u64) -> Option<u8> {
    for (section, data) in exec {
        let end = section.addr.wrapping_add(data.len() as u64);
        if target < section.addr || target >= end {
            continue;
        }
        let offset = (target - section.addr) as usize;
        let body = data.get(offset..offset + 4)?;
        if body[0] == 0x8B && body[2] == 0x24 && body[3] == 0xC3 && body[1] & 0xC7 == 0x04 {
            return Some((body[1] >> 3) & 7);
        }
        return None;
    }
    None
}

/// `.plt` 里是否存在经寄存器间接跳转的桩（`FF /4`）。找不到基址时用它区分
/// "是没解析出来"与"根本没有 PIC 桩"。
fn has_pic_stubs(exec: &[(&elf::ElfSection, &[u8])]) -> bool {
    exec.iter()
        .filter(|(section, _)| section.name.starts_with(".plt"))
        .any(|(_, data)| {
            data.windows(2)
                .any(|pair| pair[0] == 0xFF && matches!(pair[1], 0x20..=0x27 | 0xA0..=0xA7))
        })
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
        assert_eq!(
            find_got_bases(&exec),
            BTreeMap::from([(3u8, BTreeSet::from([0x3005u64]))])
        );
    }

    #[test]
    fn pc_thunk_ax_uses_the_short_add_encoding() {
        // `call __x86.get_pc_thunk.ax` 之后是 eax 的短编码 `05 imm32`（没有 81 C0）。
        let text = section(0x1000, ".text");
        let mut data = vec![0u8; 0x14];
        data[0] = 0xE8;
        data[1..5].copy_from_slice(&0x0Bu32.to_le_bytes());
        data[5] = 0x05;
        data[6..10].copy_from_slice(&0x2000u32.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&[0x8B, 0x04, 0x24, 0xC3]);
        let exec = vec![(&text, data.as_slice())];
        assert_eq!(
            find_got_bases(&exec),
            BTreeMap::from([(0u8, BTreeSet::from([0x3005u64]))])
        );
    }

    #[test]
    fn eax_relative_got_access_is_recognised() {
        // 基址在 EAX（get_pc_thunk.ax），槽位 0x3010 = 0x3005 + 0xB。
        let bases = BTreeMap::from([(0u8, BTreeSet::from([0x3005u64]))]);
        let got = BTreeMap::from([(0x3010u64, 1usize)]);
        // mov ecx, [eax+disp32]（8B 88）与 call *disp32(%eax)（FF 90）。
        let mut data = vec![0x8B, 0x88];
        data.extend_from_slice(&0x0Bu32.to_le_bytes());
        assert_eq!(taken_operand(&bases, &got, &data, 0), Some(0x3010));
        let mut data = vec![0xFF, 0x90];
        data.extend_from_slice(&0x0Bu32.to_le_bytes());
        assert_eq!(
            got_operand(&bases, &got, &data, 0),
            Some((0x3010, SiteKind::Call))
        );
        // 反例：EBX 的基址表不该被 EAX 的位移命中。
        let bases_ebx = BTreeMap::from([(3u8, BTreeSet::from([0x3005u64]))]);
        let mut data = vec![0x8B, 0x88];
        data.extend_from_slice(&0x0Bu32.to_le_bytes());
        assert_eq!(taken_operand(&bases_ebx, &got, &data, 0), None);
    }

    #[test]
    fn resolve_base_only_accepts_known_slots() {
        let bases = BTreeMap::from([(3u8, BTreeSet::from([0x3FCCu64]))]);
        let got = BTreeMap::from([(0x3FD8u64, 1usize)]);
        assert_eq!(resolve_base(&bases, &got, 3, 0xC), Some(0x3FD8));
        assert_eq!(resolve_base(&bases, &got, 3, 0x10), None);
        assert_eq!(resolve_base(&bases, &got, 0, 0xC), None);
        assert_eq!(resolve_base(&BTreeMap::new(), &got, 3, 0xC), None);
    }

    #[test]
    fn taken_operand_recognises_read_only_forms() {
        // 槽位 0x3010，基址 0x3005 ⇒ EBX 相对位移 0xB。
        let got = BTreeMap::from([(0x3010u64, 1usize)]);
        let bases = BTreeMap::from([(3u8, BTreeSet::from([0x3005u64]))]);
        let mut cases: Vec<(u8, u8, [u8; 4], bool)> = vec![
            (0x8B, 0x83, 0x0Bu32.to_le_bytes(), true), // mov reg, [ebx+disp]
            (0x8D, 0x83, 0x0Bu32.to_le_bytes(), true), // lea reg, [ebx+disp]
            (0x39, 0x83, 0x0Bu32.to_le_bytes(), true), // cmp [ebx+disp], reg
            (0x3B, 0x83, 0x0Bu32.to_le_bytes(), true), // cmp reg, [ebx+disp]
            (0x85, 0x83, 0x0Bu32.to_le_bytes(), true), // test [ebx+disp], reg
            (0xFF, 0xB3, 0x0Bu32.to_le_bytes(), true), // push [ebx+disp]
            (0x8B, 0x83, 0x10u32.to_le_bytes(), false), // 不是槽位
            (0x8B, 0x84, 0x0Bu32.to_le_bytes(), false), // mov 但不是 [ebx+disp32]
        ];
        for (first, second, disp, expected) in cases.drain(..) {
            let mut data = vec![first, second];
            data.extend_from_slice(&disp);
            let found = taken_operand(&bases, &got, &data, 0);
            assert_eq!(found.is_some(), expected, "{first:#x} {second:#x}");
        }
        // 绝对寻址：mov/cmp/test reg, [abs32]。
        for (first, second) in [(0x8B, 0x05), (0x3B, 0x05), (0x85, 0x05)] {
            let mut data = vec![first, second];
            data.extend_from_slice(&0x3010u32.to_le_bytes());
            assert_eq!(
                taken_operand(&bases, &got, &data, 0),
                Some(0x3010),
                "{first:#x} {second:#x}"
            );
        }
        // `A1` 是 `mov eax, moffs32`：地址紧跟操作码。
        let mut data = vec![0xA1];
        data.extend_from_slice(&0x3010u32.to_le_bytes());
        assert_eq!(taken_operand(&bases, &got, &data, 0), Some(0x3010));
    }

    #[test]
    fn byte_compare_and_immediate_forms_count_as_taken() {
        // 槽位 0x3010 = 基址 0x3005 + 0xB。
        let got = BTreeMap::from([(0x3010u64, 1usize)]);
        let bases = BTreeMap::from([(3u8, BTreeSet::from([0x3005u64]))]);

        // cmp r/m8|32, imm：`80/81/83 BB disp32 imm…`（EBX 相对）。
        for opcode in [0x80u8, 0x81, 0x83] {
            let mut data = vec![opcode, 0xBB];
            data.extend_from_slice(&0x0Bu32.to_le_bytes());
            data.extend_from_slice(&[0, 0, 0, 0]); // 立即数（不参与判定）
            assert_eq!(
                taken_operand(&bases, &got, &data, 0),
                Some(0x3010),
                "cmp ebx+disp, {opcode:#x}"
            );
        }
        // 同一组指令的绝对形式 `disp32`。
        for opcode in [0x80u8, 0x81, 0x83] {
            let mut data = vec![opcode, 0x3D];
            data.extend_from_slice(&0x3010u32.to_le_bytes());
            data.extend_from_slice(&[0, 0, 0, 0]);
            assert_eq!(
                taken_operand(&bases, &got, &data, 0),
                Some(0x3010),
                "cmp abs, {opcode:#x}"
            );
        }
        // mov r32, imm32：B8..BF，立即数在 +1。
        for register in 0xB8u8..=0xBF {
            let mut data = vec![register];
            data.extend_from_slice(&0x3010u32.to_le_bytes());
            assert_eq!(
                taken_operand(&bases, &got, &data, 0),
                Some(0x3010),
                "mov {register:#x}, imm32"
            );
        }
        // 反例：位移/立即数不是槽位。
        let mut data = vec![0x83, 0xBB];
        data.extend_from_slice(&0x10u32.to_le_bytes());
        data.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(taken_operand(&bases, &got, &data, 0), None);
        let mut data = vec![0xB8];
        data.extend_from_slice(&0x3014u32.to_le_bytes());
        assert_eq!(taken_operand(&bases, &got, &data, 0), None);
    }

    #[test]
    fn immediate_reference_reads_each_encoding() {
        let immediate = 0x08048160u32.to_le_bytes();
        // `push imm32` 与 `mov r32, imm32`（B8..BF）：立即数紧跟操作码。
        for opcode in [0x68u8, 0xB8, 0xBC, 0xBF] {
            let mut data = vec![opcode; 5];
            data[1..5].copy_from_slice(&immediate);
            assert_eq!(
                immediate_reference(&data, 0),
                Some(0x08048160),
                "{opcode:#x}"
            );
        }
        // `C7 /0` 的三种内存形式：立即数在 modrm + 寻址之后。
        let cases: Vec<([u8; 11], usize)> = vec![
            ([0xC7, 0x45, 0xF4, 0, 0, 0, 0, 0, 0, 0, 0], 3), // [ebp+disp8]
            ([0xC7, 0x85, 0, 0, 0, 0, 0, 0, 0, 0, 0], 6),    // [ebp+disp32]
            ([0xC7, 0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0], 6),    // [abs32]
        ];
        for (mut data, at) in cases {
            data[at..at + 4].copy_from_slice(&immediate);
            assert_eq!(
                immediate_reference(&data, 0),
                Some(0x08048160),
                "C7 at {at}"
            );
        }
        // 不认识的编码与越界。
        assert_eq!(immediate_reference(&[0x90; 6], 0), None);
        assert_eq!(immediate_reference(&[0xC7, 0x83, 0, 0, 0, 0, 0], 0), None);
        assert_eq!(immediate_reference(&[0xB8, 0, 0], 0), None);
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
