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
            let Some(at) = section.addr.checked_add(index as u64) else {
                index += 1;
                continue;
            };
            let Some(return_address) = at.checked_add(5) else {
                index += 1;
                continue;
            };
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
mod tests;
