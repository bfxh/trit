//! AArch64（ARM64）的导入交叉引用。
//!
//! 编译器访问 IAT/GOT 的典型序列是：
//!
//! ```text
//! adrp xN, page(slot)        ; 页基址
//! ldr  xN, [xN, #off]        ; xN = *slot
//! blr  xN                    ; 调用
//! ```
//!
//! 也可能在 `adrp` 与 `ldr` 之间夹一条 `add xN, xN, #off`。我们只认这三种指令
//! 组成的窗口（最多 3 条），目标槽位必须精确命中导入表，和 x86 侧同样是
//! **模式匹配而非完整解码**；寄存器经过 `mov` 搬运的情况会漏，这种情况下
//! 该导入保持"无调用点"的 `False` 是唯一诚实的结论吗？不是——所以窗口外
//! 的间接引用我们在说明里写清，不宣称覆盖全部编码。

use std::collections::{BTreeMap, BTreeSet};

use super::{RawSite, SiteKind, XrefReport, build_elf_report, mark_taken};
use crate::elf;

const ADRP_MASK: u32 = 0x9F00_0000;
const ADRP_OP: u32 = 0x9000_0000;
const ADD_IMM_MASK: u32 = 0xFFC0_0000;
const ADD_IMM_OP: u32 = 0x9100_0000;
const LDR_IMM_MASK: u32 = 0xFFC0_0000;
const LDR_IMM_OP: u32 = 0xF940_0000;
const BLR_MASK: u32 = 0xFFFF_FC1F;
const BLR_OP: u32 = 0xD63F_0000;
const BR_MASK: u32 = 0xFFFF_FC1F;
const BR_OP: u32 = 0xD61F_0000;
const BRANCH_OP_MASK: u32 = 0xFC00_0000;
/// `adrp` 与 `ldr` 之间允许夹杂的指令数（真实代码常见把参数准备插在中间）。
const MAX_ADDR_STEPS: usize = 12;
/// `ldr` 与 `blr`/`br` 之间允许夹杂的指令数。
const MAX_CALL_STEPS: usize = 8;
const BL_OP: u32 = 0x9400_0000;
const B_OP: u32 = 0x1400_0000;

/// 一次间接引用的候选。
#[derive(Debug, Clone, Copy)]
struct Reference {
    /// `blr` 的地址（直接调用）或 `adrp` 的地址（跳板起点）。
    site: u64,
    /// 计算出的槽位地址。
    slot: u64,
    kind: SiteKind,
}

fn sign_extend(value: i64, bits: u32) -> i64 {
    let shift = 64 - bits;
    (value << shift) >> shift
}

fn instructions(data: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(data.len() / 4);
    let mut index = 0usize;
    while index + 4 <= data.len() {
        out.push(u32::from_le_bytes([
            data[index],
            data[index + 1],
            data[index + 2],
            data[index + 3],
        ]));
        index += 4;
    }
    out
}

fn adrp_page(insn: u32, pc: u64) -> Option<u64> {
    if insn & ADRP_MASK != ADRP_OP {
        return None;
    }
    let immlo = ((insn >> 29) & 0x3) as i64;
    let immhi = ((insn >> 5) & 0x7_FFFF) as i64;
    let imm = sign_extend((immhi << 2) | immlo, 21);
    Some((pc & !0xFFF).wrapping_add((imm << 12) as u64))
}

fn add_imm(insn: u32, register: u32) -> Option<(u32, u64)> {
    if insn & ADD_IMM_MASK != ADD_IMM_OP {
        return None;
    }
    let shift = (insn >> 22) & 0x3;
    if shift > 1 {
        return None;
    }
    if (insn >> 5) & 0x1F != register {
        return None;
    }
    let imm12 = (insn >> 10) & 0xFFF;
    Some(((insn & 0x1F), (imm12 as u64) << (12 * shift)))
}

fn ldr_imm(insn: u32, register: u32) -> Option<(u32, u64)> {
    if insn & LDR_IMM_MASK != LDR_IMM_OP {
        return None;
    }
    if (insn >> 5) & 0x1F != register {
        return None;
    }
    let imm12 = (insn >> 10) & 0xFFF;
    Some((insn & 0x1F, imm12 as u64 * 8))
}

/// `mov Xd, Xm`（ORR 别名）：返回 `(目标, 源)`。
fn mov_alias(insn: u32) -> Option<(u32, u32)> {
    // ORR (shifted register) 的 MOV 别名：Rn=31、shift=0、imm6=0（sf 位忽略）。
    if insn & 0x7FE0_FFE0 == 0x2A00_03E0 {
        Some((insn & 0x1F, (insn >> 16) & 0x1F))
    } else {
        None
    }
}

fn blr_register(insn: u32) -> Option<u32> {
    if insn & BLR_MASK == BLR_OP {
        Some((insn >> 5) & 0x1F)
    } else {
        None
    }
}

fn br_register(insn: u32) -> Option<u32> {
    if insn & BR_MASK == BR_OP {
        Some((insn >> 5) & 0x1F)
    } else {
        None
    }
}

/// `bl`/`b` 的目标；第二个返回值表示是否为 `bl`（带链接）。
fn branch_target(insn: u32, pc: u64) -> Option<(u64, bool)> {
    let opcode = insn & BRANCH_OP_MASK;
    if opcode != BL_OP && opcode != B_OP {
        return None;
    }
    let imm = sign_extend(((insn & 0x03FF_FFFF) as i64) << 2, 28);
    Some((pc.wrapping_add(imm as u64), opcode == BL_OP))
}

/// 这条指令是否写 `register`：`Some(true/false)` 是能判定的常见类别，
/// `None` 表示"不认识"——调用方按**覆盖**处理（宁可断链，不给错归属）。
fn writes(insn: u32, register: u32) -> Option<bool> {
    let rd = insn & 0x1F;
    let rt2 = (insn >> 10) & 0x1F;
    // ADR / ADRP
    if insn & 0x9F00_0000 == 0x9000_0000 || insn & 0x9F00_0000 == 0x1000_0000 {
        return Some(rd == register);
    }
    // ADD/SUB（立即数或寄存器）、逻辑移位运算、位域、条件选择：目标在 Rd。
    if insn & 0x1F00_0000 == 0x1100_0000
        || insn & 0x1F00_0000 == 0x0B00_0000
        || insn & 0x1F00_0000 == 0x0A00_0000
        || insn & 0x1F80_0000 == 0x1300_0000
        || insn & 0x1FE0_0000 == 0x1A80_0000
    {
        return Some(rd == register);
    }
    // MOVZ / MOVK
    if insn & 0x7F80_0000 == 0x5280_0000 || insn & 0x7F80_0000 == 0x7280_0000 {
        return Some(rd == register);
    }
    // 载入/存储：无符号偏移、非缩放偏移、寄存器偏移、字面量。
    if insn & 0x3B00_0000 == 0x3900_0000
        || insn & 0x3B00_0000 == 0x3800_0000
        || insn & 0x3B00_0000 == 0x1800_0000
    {
        return Some(rd == register);
    }
    // LDP/STP：Rt 与 Rt2 都算写。
    if insn & 0x3A00_0000 == 0x2800_0000 {
        return Some(rd == register || rt2 == register);
    }
    // 调用会破坏调用者保存寄存器 x0–x18 与 x30；x19–x29 跨调用保留。
    let opcode = insn & 0xFC00_0000;
    if opcode == 0x9400_0000 {
        return Some(register <= 18 || register == 30);
    }
    if insn & 0xFFFF_FC1F == 0xD63F_0000 {
        return Some(register <= 18 || register == 30);
    }
    // 无条件跳转/返回：本路径到此为止，后续指令不可达，按"未知"断链。
    if opcode == 0x1400_0000
        || insn & 0xFFFF_FC1F == 0xD61F_0000
        || insn & 0xFFFF_FC1F == 0xD65F_0000
    {
        return None;
    }
    // HINT / 屏障 / PAC：不写通用寄存器；MRS 写 Rt，MSR 只读 Rt。
    if insn & 0xFFFF_0000 == 0xD503_0000 {
        return Some(false);
    }
    if insn & 0xFFF0_0000 == 0xD530_0000 {
        return Some(rd == register);
    }
    if insn & 0xFFF0_0000 == 0xD510_0000 {
        return Some(false);
    }
    None
}

/// 扫描一段代码里的 `adrp + [add] + ldr + blr/br` 序列。
///
/// 允许中间夹杂其他指令（参数准备很常见），但会被对目标寄存器的破坏性写入打断。
/// 返回值第二项是"槽位地址被计算过但没有直接调用"的槽位——它们不能被判成"没有引用"。
fn references(data: &[u8], base: u64) -> (Vec<Reference>, BTreeSet<u64>) {
    let insns = instructions(data);
    let mut out = Vec::new();
    let mut taken = BTreeSet::new();
    for index in 0..insns.len() {
        let pc = base + index as u64 * 4;
        let Some(page) = adrp_page(insns[index], pc) else {
            continue;
        };
        let mut register = insns[index] & 0x1F;
        let mut address = page;
        let mut cursor = index + 1;
        let mut steps = 1usize;
        let mut loaded = None;
        while cursor < insns.len() && steps < MAX_ADDR_STEPS {
            let insn = insns[cursor];
            if let Some((next_register, immediate)) = add_imm(insn, register) {
                address = address.wrapping_add(immediate);
                register = next_register;
                cursor += 1;
                steps += 1;
                continue;
            }
            if let Some((target_register, offset)) = ldr_imm(insn, register) {
                loaded = Some((target_register, offset));
                cursor += 1;
                break;
            }
            if let Some((dst, src)) = mov_alias(insn)
                && src == register
            {
                register = dst;
                cursor += 1;
                steps += 1;
                continue;
            }
            match writes(insn, register) {
                Some(false) => {
                    cursor += 1;
                    steps += 1;
                }
                _ => break,
            }
        }
        let Some((target_register, offset)) = loaded else {
            continue;
        };
        let slot = address.wrapping_add(offset);
        let mut steps = 0usize;
        let mut called = false;
        let mut call_register = target_register;
        while cursor < insns.len() && steps < MAX_CALL_STEPS {
            let insn = insns[cursor];
            let at = base + cursor as u64 * 4;
            if let Some((dst, src)) = mov_alias(insn)
                && src == call_register
            {
                call_register = dst;
                cursor += 1;
                steps += 1;
                continue;
            }
            if blr_register(insn) == Some(call_register) {
                out.push(Reference {
                    site: at,
                    slot,
                    kind: SiteKind::Call,
                });
                called = true;
                break;
            }
            if br_register(insn) == Some(call_register) {
                out.push(Reference {
                    site: pc,
                    slot,
                    kind: SiteKind::Jump,
                });
                called = true;
                break;
            }
            match writes(insn, call_register) {
                Some(false) => {
                    cursor += 1;
                    steps += 1;
                }
                _ => break,
            }
        }
        if !called {
            taken.insert(slot);
        }
    }
    (out, taken)
}

/// PE ARM64：直接调用 + 跳板展开。
pub(super) fn scan_pe_sections(
    exec: &[(u32, usize, &[u8])],
    slots: &BTreeMap<u64, usize>,
    sites: &mut [BTreeSet<RawSite>],
    taken: &mut BTreeSet<u64>,
) {
    for (section_rva, file_base, data) in exec {
        let base = *section_rva as u64;
        let (references, referenced) = references(data, base);
        for reference in references {
            if let Some(&row) = slots.get(&reference.slot) {
                let offset = (reference.site - base) as usize;
                sites[row].insert((reference.site, reference.kind, file_base + offset));
            }
        }
        for slot in referenced {
            if slots.contains_key(&slot) {
                taken.insert(slot);
            }
        }
    }
    expand_thunks(exec, sites);
}

fn expand_thunks(exec: &[(u32, usize, &[u8])], sites: &mut [BTreeSet<RawSite>]) {
    let mut thunks: BTreeMap<u64, usize> = BTreeMap::new();
    for (row, set) in sites.iter().enumerate() {
        for (address, kind, _) in set {
            if *kind == SiteKind::Jump {
                thunks.insert(*address, row);
            }
        }
    }
    if thunks.is_empty() {
        return;
    }
    let mut callers: BTreeMap<usize, BTreeSet<RawSite>> = BTreeMap::new();
    for (section_rva, file_base, data) in exec {
        let base = *section_rva as u64;
        for (index, insn) in instructions(data).iter().enumerate() {
            let pc = base + index as u64 * 4;
            let Some((target, is_call)) = branch_target(*insn, pc) else {
                continue;
            };
            if let Some(row) = thunks.get(&target) {
                let kind = if is_call {
                    SiteKind::Call
                } else {
                    SiteKind::Jump
                };
                callers
                    .entry(*row)
                    .or_default()
                    .insert((pc, kind, file_base + index * 4));
            }
        }
    }
    for (row, found) in callers {
        sites[row].retain(|(_, kind, _)| *kind != SiteKind::Jump);
        sites[row].extend(found);
    }
}

/// ELF ARM64：PLT 桩（`adrp+ldr+br`）+ `bl` 调用方 + 直接 `blr` 经 GOT。
pub(super) fn scan_elf(
    arch: &str,
    symbols: &[elf::DynamicSymbol],
    jump_slots: &[(u64, usize)],
    exec_data: &[(&elf::ElfSection, &[u8])],
    complete: bool,
    scanned: usize,
) -> XrefReport {
    let mut symbol_of_got: BTreeMap<u64, usize> = BTreeMap::new();
    for (got, symbol) in jump_slots {
        symbol_of_got.entry(*got).or_insert(*symbol);
    }

    let mut stub_to_symbol: BTreeMap<u64, usize> = BTreeMap::new();
    let mut stub_ranges: Vec<(u64, u64)> = Vec::new();
    let mut taken: BTreeSet<u64> = BTreeSet::new();
    for (section, data) in exec_data {
        let (references, referenced) = references(data, section.addr);
        for slot in referenced {
            taken.insert(slot);
        }
        for reference in references {
            if reference.kind != SiteKind::Jump {
                continue;
            }
            if let Some(&symbol) = symbol_of_got.get(&reference.slot) {
                let stub = reference.site & !0xF;
                stub_to_symbol.entry(stub).or_insert(symbol);
                stub_ranges.push((stub, stub + 16));
            }
        }
    }

    let mut sites: BTreeMap<usize, BTreeSet<RawSite>> = BTreeMap::new();
    for (section, data) in exec_data {
        let base = section.addr;
        let (references, _) = references(data, base);
        for reference in references {
            if reference.kind != SiteKind::Call {
                continue;
            }
            if let Some(&symbol) = symbol_of_got.get(&reference.slot) {
                let offset = (reference.site - base) as usize;
                sites.entry(symbol).or_default().insert((
                    reference.site,
                    SiteKind::Call,
                    section.offset as usize + offset,
                ));
            }
        }
        for (index, insn) in instructions(data).iter().enumerate() {
            let pc = base + index as u64 * 4;
            if stub_ranges
                .iter()
                .any(|(low, high)| pc >= *low && pc < *high)
            {
                continue;
            }
            let Some((target, is_call)) = branch_target(*insn, pc) else {
                continue;
            };
            if let Some(&symbol) = stub_to_symbol.get(&target) {
                let kind = if is_call {
                    SiteKind::Call
                } else {
                    SiteKind::Jump
                };
                sites.entry(symbol).or_default().insert((
                    pc,
                    kind,
                    section.offset as usize + index * 4,
                ));
            }
        }
    }

    let mut report = build_elf_report(arch, symbols, jump_slots, &sites, complete, scanned);
    mark_taken(&mut report.imports, &taken);
    report
}

#[cfg(test)]
mod tests {
    use super::{SiteKind, adrp_page, branch_target, ldr_imm, references};

    fn adrp(pc: u64, target: u64, register: u32) -> u32 {
        let delta = ((target & !0xFFF) as i64 - (pc & !0xFFF) as i64) >> 12;
        let imm = delta as u32 & 0x1F_FFFF;
        let immlo = (imm & 0x3) << 29;
        let immhi = ((imm >> 2) & 0x7_FFFF) << 5;
        0x9000_0000 | immlo | immhi | register
    }

    fn ldr(register: u32, offset: u64, target: u32) -> u32 {
        assert_eq!(offset % 8, 0);
        0xF940_0000 | (((offset / 8) as u32) << 10) | (register << 5) | target
    }

    fn add(register: u32, offset: u64, target: u32) -> u32 {
        0x9100_0000 | ((offset as u32) << 10) | (register << 5) | target
    }

    fn blr(register: u32) -> u32 {
        0xD63F_0000 | (register << 5)
    }

    fn br(register: u32) -> u32 {
        0xD61F_0000 | (register << 5)
    }

    fn bl(pc: u64, target: u64) -> u32 {
        let delta = ((target as i64 - pc as i64) >> 2) as u32 & 0x03FF_FFFF;
        0x9400_0000 | delta
    }

    fn put(data: &mut [u8], index: usize, insn: u32) {
        data[index * 4..index * 4 + 4].copy_from_slice(&insn.to_le_bytes());
    }

    #[test]
    fn decodes_adrp_page() {
        // 0x1000 → 0x2000
        assert_eq!(adrp_page(adrp(0x1000, 0x2000, 9), 0x1000), Some(0x2000));
        // 负方向
        let insn = adrp(0x2000, 0x1000, 3);
        assert_eq!(adrp_page(insn, 0x2000), Some(0x1000));
        assert_eq!(adrp_page(0xDEAD_BEEF, 0x1000), None);
    }

    #[test]
    fn finds_direct_blr_through_slot() {
        let mut data = vec![0u8; 16];
        put(&mut data, 0, adrp(0x1000, 0x2000, 9));
        put(&mut data, 1, ldr(9, 0x40, 9));
        put(&mut data, 2, blr(9));
        let (refs, _) = references(&data, 0x1000);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].slot, 0x2040);
        assert_eq!(refs[0].site, 0x1008);
        assert_eq!(refs[0].kind, SiteKind::Call);
    }

    #[test]
    fn follows_add_then_ldr() {
        let mut data = vec![0u8; 16];
        put(&mut data, 0, adrp(0x1000, 0x2000, 9));
        put(&mut data, 1, add(9, 0x48, 9));
        put(&mut data, 2, ldr(9, 0x0, 9));
        put(&mut data, 3, blr(9));
        let (refs, _) = references(&data, 0x1000);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].slot, 0x2048);
        assert_eq!(refs[0].site, 0x100c);
    }

    #[test]
    fn interleaved_instructions_do_not_hide_the_reference() {
        let mut data = vec![0u8; 40];
        put(&mut data, 0, adrp(0x1000, 0x2000, 9));
        put(&mut data, 1, 0xD2800020); // movz x0, #1
        put(&mut data, 2, ldr(9, 0x40, 9));
        put(&mut data, 3, 0xD2800041); // movz x1, #2
        put(&mut data, 4, 0xD2800062); // movz x2, #3
        put(&mut data, 5, blr(9));
        let (refs, _) = references(&data, 0x1000);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].slot, 0x2040);
        assert_eq!(refs[0].site, 0x1014);
    }

    fn mov(dst: u32, src: u32) -> u32 {
        0xAA00_03E0 | (src << 16) | dst
    }

    #[test]
    fn mov_alias_carries_the_address() {
        let mut data = vec![0u8; 20];
        put(&mut data, 0, adrp(0x1000, 0x2000, 9));
        put(&mut data, 1, mov(10, 9));
        put(&mut data, 2, ldr(10, 0x40, 10));
        put(&mut data, 3, 0xD503201F); // nop
        put(&mut data, 4, blr(10));
        let (refs, _) = references(&data, 0x1000);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].slot, 0x2040);
        assert_eq!(refs[0].site, 0x1010);
    }

    #[test]
    fn mov_alias_carries_the_loaded_pointer() {
        let mut data = vec![0u8; 20];
        put(&mut data, 0, adrp(0x1000, 0x2000, 9));
        put(&mut data, 1, ldr(9, 0x40, 9));
        put(&mut data, 2, mov(10, 9));
        put(&mut data, 3, 0xD503201F); // nop
        put(&mut data, 4, blr(10));
        let (refs, _) = references(&data, 0x1000);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].slot, 0x2040);
    }

    #[test]
    fn clobbered_register_breaks_the_chain() {
        let mut data = vec![0u8; 16];
        put(&mut data, 0, adrp(0x1000, 0x2000, 9));
        put(&mut data, 1, 0xD2800009); // movz x9, #0
        put(&mut data, 2, ldr(9, 0x40, 9));
        put(&mut data, 3, blr(9));
        let (refs, _) = references(&data, 0x1000);
        assert!(refs.is_empty());
    }

    #[test]
    fn address_only_reference_is_marked_taken() {
        let mut data = vec![0u8; 24];
        put(&mut data, 0, adrp(0x1000, 0x2000, 9));
        put(&mut data, 1, ldr(9, 0x40, 9));
        let (refs, taken) = references(&data, 0x1000);
        assert!(refs.is_empty());
        assert!(taken.contains(&0x2040));
    }

    #[test]
    fn thunk_is_a_jump_at_adrp() {
        let mut data = vec![0u8; 12];
        put(&mut data, 0, adrp(0x1020, 0x2000, 16));
        put(&mut data, 1, ldr(16, 0x40, 17));
        put(&mut data, 2, br(17));
        let (refs, _) = references(&data, 0x1020);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, SiteKind::Jump);
        assert_eq!(refs[0].site, 0x1020);
        assert_eq!(refs[0].slot, 0x2040);
    }

    #[test]
    fn branch_targets_are_signed() {
        let insn = bl(0x2000, 0x1000);
        let (target, is_call) = branch_target(insn, 0x2000).unwrap();
        assert!(is_call);
        assert_eq!(target, 0x1000);
        assert_eq!(ldr_imm(ldr(9, 0x40, 9), 9), Some((9, 0x40)));
    }
}
