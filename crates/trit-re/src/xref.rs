//! 导入函数的静态交叉引用（xref）。
//!
//! 回答一个问题：**每个导入函数在二进制里有没有被调用点**。结论是三值的：
//! 找到 → `True`；完整扫过可执行节且没找到 → `False`；架构不支持/节数据越界/
//! 重定位缺失 → `Unknown`。
//!
//! 扫描方式是**字节模式匹配，不是完整反汇编**：
//! - PE x86-64：`FF /2`(call)、`FF /4`(jmp) 的 RIP 相对编码；
//! - PE x86：同两个操作码的 32 位绝对寻址编码；
//! - ELF x86-64：`.rela.plt` 的 `JUMP_SLOT` → GOT 槽位，PLT 桩里的 `FF /4`，
//!   以及调用方的 `E8`(call rel32)、`FF /2` 经 GOT。
//!
//! 因此**数据段里恰好长得像指令的字节仍可能被误报**。降低误报的唯一手段是
//! 目标地址必须精确等于某个导入槽位；做不到这一点的地方一律不给 `False`。

use std::collections::{BTreeMap, BTreeSet};

use trit_core::value::Ternary;

use crate::elf;
use crate::pe;

/// 单次扫描的字节上限，防止超大文件把 CLI 拖死。
pub const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;
/// 单个导入最多保留的调用点，超过只保留前 N 个。
pub const MAX_SITES: usize = 4096;

/// 调用点类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SiteKind {
    /// `call`。
    Call,
    /// `jmp`（尾调用 / PLT 跳板）。
    Jump,
}

impl SiteKind {
    /// 稳定短名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Jump => "jump",
        }
    }
}

/// 一个调用点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    /// PE 为 RVA，ELF 为虚拟地址。
    pub addr: u64,
    /// 文件内偏移。
    pub file_offset: usize,
    /// 调用类型。
    pub kind: SiteKind,
}

/// 单个导入的交叉引用结果。
#[derive(Debug, Clone)]
pub struct ImportXref {
    /// `DLL!Func`（PE）或符号名（ELF）。
    pub import: String,
    /// PE 为 IAT 槽位 RVA；ELF 为 GOT 槽位虚拟地址；没有槽位时为 0。
    pub slot: u64,
    /// 调用点（去重、升序）。
    pub sites: Vec<CallSite>,
    /// 三值结论。
    pub status: Ternary,
    /// 证据说明。
    pub note: String,
}

impl ImportXref {
    /// 单字符状态：`T` / `F` / `?`。
    pub fn status_char(&self) -> char {
        match self.status {
            Ternary::True => 'T',
            Ternary::False => 'F',
            Ternary::Unknown => '?',
        }
    }
}

/// 一次交叉引用扫描的报告。
#[derive(Debug, Clone)]
pub struct XrefReport {
    /// 架构短名。
    pub arch: String,
    /// 实际扫描字节数。
    pub scanned_bytes: usize,
    /// 是否完整扫过全部可执行节。
    pub complete: bool,
    /// 每个导入一行。
    pub imports: Vec<ImportXref>,
    /// 扫描层面的说明（不参与三值判定）。
    pub notes: Vec<String>,
}

impl XrefReport {
    /// 有调用点的导入数。
    pub fn referenced(&self) -> usize {
        self.imports
            .iter()
            .filter(|row| row.status == Ternary::True)
            .count()
    }

    /// 完整扫描且无调用点的导入数。
    pub fn unreferenced(&self) -> usize {
        self.imports
            .iter()
            .filter(|row| row.status == Ternary::False)
            .count()
    }

    /// 证据不足的导入数。
    pub fn unknown(&self) -> usize {
        self.imports
            .iter()
            .filter(|row| row.status == Ternary::Unknown)
            .count()
    }
}

/// 一个内部调用点：地址、类型、文件偏移。
type RawSite = (u64, SiteKind, usize);

fn empty_report(arch: &str, note: &str) -> XrefReport {
    XrefReport {
        arch: arch.to_string(),
        scanned_bytes: 0,
        complete: false,
        imports: Vec::new(),
        notes: vec![note.to_string()],
    }
}

fn unknown_rows(names: impl Iterator<Item = String>, note: &str) -> Vec<ImportXref> {
    names
        .map(|import| ImportXref {
            import,
            slot: 0,
            sites: Vec::new(),
            status: Ternary::Unknown,
            note: note.to_string(),
        })
        .collect()
}

fn to_sites(set: &BTreeSet<RawSite>) -> Vec<CallSite> {
    set.iter()
        .take(MAX_SITES)
        .map(|(addr, kind, file_offset)| CallSite {
            addr: *addr,
            file_offset: *file_offset,
            kind: *kind,
        })
        .collect()
}

fn finalize(row: &mut ImportXref, complete: bool) {
    row.status = if !row.sites.is_empty() {
        Ternary::True
    } else if complete && row.note.is_empty() {
        Ternary::False
    } else {
        Ternary::Unknown
    };
    if row.status == Ternary::True && row.note.is_empty() {
        let calls = row
            .sites
            .iter()
            .filter(|site| site.kind == SiteKind::Call)
            .count();
        row.note = format!("{} 个调用点（{} call）", row.sites.len(), calls);
    }
}

fn slice_at(bytes: &[u8], start: usize, len: usize) -> Option<&[u8]> {
    if len == 0 {
        return Some(&[]);
    }
    bytes.get(start..start.checked_add(len)?)
}

/// 扫描 PE 的 IAT 交叉引用。
pub fn scan_pe(bytes: &[u8], info: &pe::PeInfo) -> XrefReport {
    let arch = info.arch();
    if !matches!(info.machine, 0x8664 | 0x014c) {
        let mut report = empty_report(arch, "只支持 x86 / x86-64 的 IAT 模式扫描");
        if let Some(dlls) = &info.imports {
            report.imports = unknown_rows(
                dlls.iter().flat_map(|dll| {
                    dll.functions
                        .iter()
                        .map(move |function| format!("{}!{}", dll.name, function.name))
                }),
                "架构不支持，无法判定调用点",
            );
        }
        return report;
    }
    let Some(dlls) = &info.imports else {
        return empty_report(arch, "导入表不可信，无法建立槽位表");
    };

    let mut slots: BTreeMap<u64, usize> = BTreeMap::new();
    let mut rows: Vec<ImportXref> = Vec::new();
    let mut slot_unknown = false;
    for dll in dlls {
        for function in &dll.functions {
            let import = format!("{}!{}", dll.name, function.name);
            match function.slot_rva {
                Some(rva) => {
                    slots.insert(info.image_base.wrapping_add(rva as u64), rows.len());
                    rows.push(ImportXref {
                        import,
                        slot: rva as u64,
                        sites: Vec::new(),
                        status: Ternary::Unknown,
                        note: String::new(),
                    });
                }
                None => {
                    slot_unknown = true;
                    rows.push(ImportXref {
                        import,
                        slot: 0,
                        sites: Vec::new(),
                        status: Ternary::Unknown,
                        note: "FirstThunk 缺失，IAT 槽位未知".to_string(),
                    });
                }
            }
        }
    }

    let mut report = XrefReport {
        arch: arch.to_string(),
        scanned_bytes: 0,
        complete: !slot_unknown,
        imports: rows,
        notes: Vec::new(),
    };
    if slot_unknown {
        report
            .notes
            .push("部分导入缺少 FirstThunk，这些导入无法定位槽位".to_string());
    }

    let mut sites: Vec<BTreeSet<RawSite>> = vec![BTreeSet::new(); report.imports.len()];
    let mut exec: Vec<(u32, usize, &[u8])> = Vec::new();
    for section in &info.sections {
        if !section.executable() {
            continue;
        }
        let Some(data) = slice_at(bytes, section.raw_ptr as usize, section.raw_size as usize)
        else {
            report.complete = false;
            report
                .notes
                .push(format!("节 {} 的数据越界，交叉引用不完整", section.name));
            continue;
        };
        if report.scanned_bytes + data.len() > MAX_SCAN_BYTES {
            report.complete = false;
            report.notes.push(format!(
                "超过 {} MiB 扫描上限，交叉引用不完整",
                MAX_SCAN_BYTES / 1024 / 1024
            ));
            break;
        }
        scan_pe_section(
            data,
            section.virtual_address,
            section.raw_ptr as usize,
            info.plus,
            info.image_base,
            &slots,
            &mut sites,
        );
        report.scanned_bytes += data.len();
        exec.push((section.virtual_address, section.raw_ptr as usize, data));
    }

    // 第二层：导入跳板（`jmp [IAT]`）会被调用方用 `E8`/`E9` 直接调用。
    // 找到调用方就以调用方为准，避免把跳板本身算成"归不到函数"的调用点。
    expand_pe_thunks(&exec, &mut sites);

    for (index, row) in report.imports.iter_mut().enumerate() {
        row.sites = to_sites(&sites[index]);
        finalize(row, report.complete);
    }
    report
}

#[allow(clippy::too_many_arguments)]
fn scan_pe_section(
    data: &[u8],
    section_rva: u32,
    file_base: usize,
    plus: bool,
    image_base: u64,
    slots: &BTreeMap<u64, usize>,
    sites: &mut [BTreeSet<RawSite>],
) {
    let mut index = 0usize;
    while index + 6 <= data.len() {
        if data[index] == 0xFF && (data[index + 1] == 0x15 || data[index + 1] == 0x25) {
            let disp = i32::from_le_bytes([
                data[index + 2],
                data[index + 3],
                data[index + 4],
                data[index + 5],
            ]);
            let target = if plus {
                image_base
                    .wrapping_add(section_rva as u64 + index as u64 + 6)
                    .wrapping_add(disp as i64 as u64)
            } else {
                (disp as u32) as u64
            };
            if let Some(&row) = slots.get(&target) {
                let kind = if data[index + 1] == 0x15 {
                    SiteKind::Call
                } else {
                    SiteKind::Jump
                };
                sites[row].insert((section_rva as u64 + index as u64, kind, file_base + index));
            }
        }
        index += 1;
    }
}

/// 把 `jmp [IAT]` 跳板展开成真正的调用方。
fn expand_pe_thunks(exec: &[(u32, usize, &[u8])], sites: &mut [BTreeSet<RawSite>]) {
    let mut thunks: BTreeMap<u64, usize> = BTreeMap::new();
    for (row, set) in sites.iter().enumerate() {
        for (addr, kind, _) in set {
            if *kind == SiteKind::Jump {
                thunks.insert(*addr, row);
            }
        }
    }
    if thunks.is_empty() {
        return;
    }
    let mut callers: BTreeMap<usize, BTreeSet<RawSite>> = BTreeMap::new();
    for (section_rva, file_base, data) in exec {
        let mut index = 0usize;
        while index + 5 <= data.len() {
            if data[index] == 0xE8 || data[index] == 0xE9 {
                let rel = i32::from_le_bytes([
                    data[index + 1],
                    data[index + 2],
                    data[index + 3],
                    data[index + 4],
                ]);
                let here = *section_rva as u64 + index as u64;
                let target = here.wrapping_add(5).wrapping_add(rel as i64 as u64);
                if let Some(row) = thunks.get(&target) {
                    let kind = if data[index] == 0xE8 {
                        SiteKind::Call
                    } else {
                        SiteKind::Jump
                    };
                    callers
                        .entry(*row)
                        .or_default()
                        .insert((here, kind, file_base + index));
                }
            }
            index += 1;
        }
    }
    for (row, found) in callers {
        sites[row].retain(|(_, kind, _)| *kind != SiteKind::Jump);
        sites[row].extend(found);
    }
}

/// 扫描 ELF x86-64 的 PLT/GOT 交叉引用。
pub fn scan_elf(bytes: &[u8], info: &elf::ElfInfo) -> XrefReport {
    let arch = info.arch();
    if !(info.class == 2 && info.machine == 62) {
        let mut report = empty_report(arch, "只支持 x86-64 的 PLT/GOT 交叉引用");
        if let Some(imports) = &info.imports {
            report.imports = unknown_rows(imports.iter().cloned(), "架构不支持，无法判定调用点");
        }
        return report;
    }
    let Some(symbols) = info.dynamic_symbols(bytes) else {
        let mut report = empty_report(arch, "无节表，读不到动态符号与重定位");
        if let Some(imports) = &info.imports {
            report.imports = unknown_rows(imports.iter().cloned(), "无节表，无法定位 GOT 槽位");
        }
        return report;
    };
    let Some(jump_slots) = elf_jump_slots(bytes, info, &symbols) else {
        let mut report = empty_report(arch, "没有 JUMP_SLOT 重定位（静态链接或节表缺失）");
        if let Some(imports) = &info.imports {
            report.imports = unknown_rows(imports.iter().cloned(), "没有 JUMP_SLOT 重定位");
        }
        return report;
    };

    let mut symbol_of_got: BTreeMap<u64, usize> = BTreeMap::new();
    for (got, symbol) in &jump_slots {
        symbol_of_got.entry(*got).or_insert(*symbol);
    }

    let mut complete = true;
    let mut scanned = 0usize;
    let executable: Vec<&elf::ElfSection> = info
        .sections
        .iter()
        .filter(|section| section.flags & 0x4 != 0 && section.size > 0)
        .collect();
    let mut exec_data: Vec<(&elf::ElfSection, &[u8])> = Vec::new();
    for section in &executable {
        let Some(data) = slice_at(bytes, section.offset as usize, section.size as usize) else {
            complete = false;
            continue;
        };
        if scanned + data.len() > MAX_SCAN_BYTES {
            complete = false;
            break;
        }
        scanned += data.len();
        exec_data.push((section, data));
    }

    // 第一遍：PLT 桩（`FF /4` 经 GOT）→ 桩地址 → 符号。
    let mut stub_to_symbol: BTreeMap<u64, usize> = BTreeMap::new();
    let mut stub_instructions: BTreeSet<u64> = BTreeSet::new();
    for (section, data) in &exec_data {
        let mut index = 0usize;
        while index + 6 <= data.len() {
            if data[index] == 0xFF && data[index + 1] == 0x25 {
                let disp = i32::from_le_bytes([
                    data[index + 2],
                    data[index + 3],
                    data[index + 4],
                    data[index + 5],
                ]);
                let target = section
                    .addr
                    .wrapping_add(index as u64 + 6)
                    .wrapping_add(disp as i64 as u64);
                if let Some(symbol) = symbol_of_got.get(&target) {
                    let instruction = section.addr + index as u64;
                    stub_to_symbol.entry(instruction & !0xF).or_insert(*symbol);
                    stub_instructions.insert(instruction);
                }
            }
            index += 1;
        }
    }

    // 第二遍：调用方。
    let mut sites: BTreeMap<usize, BTreeSet<RawSite>> = BTreeMap::new();
    for (section, data) in &exec_data {
        let mut index = 0usize;
        while index + 5 <= data.len() {
            let here = section.addr + index as u64;
            if stub_instructions.contains(&here) {
                index += 1;
                continue;
            }
            let file_offset = section.offset as usize + index;
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
                    sites
                        .entry(*symbol)
                        .or_default()
                        .insert((here, kind, file_offset));
                }
            }
            if index + 6 <= data.len() && data[index] == 0xFF && data[index + 1] == 0x15 {
                let disp = i32::from_le_bytes([
                    data[index + 2],
                    data[index + 3],
                    data[index + 4],
                    data[index + 5],
                ]);
                let target = here.wrapping_add(6).wrapping_add(disp as i64 as u64);
                if let Some(symbol) = symbol_of_got.get(&target) {
                    sites
                        .entry(*symbol)
                        .or_default()
                        .insert((here, SiteKind::Call, file_offset));
                }
            }
            index += 1;
        }
    }

    let mut rows = Vec::new();
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    for (got, symbol) in &jump_slots {
        if !seen.insert(*symbol) {
            continue;
        }
        let Some(symbol_info) = symbols.iter().find(|entry| entry.index == *symbol) else {
            continue;
        };
        let mut row = ImportXref {
            import: symbol_info.name.clone(),
            slot: *got,
            sites: sites.get(symbol).map(to_sites).unwrap_or_default(),
            status: Ternary::Unknown,
            note: String::new(),
        };
        finalize(&mut row, complete);
        rows.push(row);
    }
    for symbol in symbols.iter().filter(|entry| !entry.defined) {
        if !seen.contains(&symbol.index) {
            rows.push(ImportXref {
                import: symbol.name.clone(),
                slot: 0,
                sites: Vec::new(),
                status: if complete {
                    Ternary::False
                } else {
                    Ternary::Unknown
                },
                note: "没有 JUMP_SLOT 重定位".to_string(),
            });
        }
    }

    let mut report = XrefReport {
        arch: arch.to_string(),
        scanned_bytes: scanned,
        complete,
        imports: rows,
        notes: Vec::new(),
    };
    if !complete {
        report
            .notes
            .push("有可执行节读不出或超过扫描上限，交叉引用不完整".to_string());
    }
    report
}

/// 解析 ELF 的 `JUMP_SLOT` 重定位：`(GOT 虚拟地址, 符号下标)`。
fn elf_jump_slots(
    bytes: &[u8],
    info: &elf::ElfInfo,
    symbols: &[elf::DynamicSymbol],
) -> Option<Vec<(u64, usize)>> {
    let known: BTreeSet<usize> = symbols.iter().map(|symbol| symbol.index).collect();
    let mut slots = Vec::new();
    let mut found_table = false;
    for section in &info.sections {
        if section.sh_type != 4 && section.sh_type != 9 {
            continue;
        }
        // RELA 24 字节 / REL 16 字节，只认 x86-64 的 JUMP_SLOT(=7)。
        let entry_size = if section.sh_type == 4 { 24 } else { 16 };
        let count = section.size as usize / entry_size;
        let Some(data) = slice_at(bytes, section.offset as usize, count * entry_size) else {
            continue;
        };
        for index in 0..count {
            let base = index * entry_size;
            let got = u64::from_le_bytes(data[base..base + 8].try_into().ok()?);
            let info_word = u64::from_le_bytes(data[base + 8..base + 16].try_into().ok()?);
            let kind = info_word as u32;
            let symbol = (info_word >> 32) as usize;
            if kind == 7 && known.contains(&symbol) {
                found_table = true;
                slots.push((got, symbol));
            }
        }
    }
    if found_table { Some(slots) } else { None }
}

#[cfg(test)]
mod tests {
    use super::{SiteKind, scan_pe};
    use crate::pe;
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
        file[0x44..0x46].copy_from_slice(&0xaa64u16.to_le_bytes());
        let info = pe::parse(&file).expect("parse");
        let report = scan_pe(&file, &info);
        assert!(!report.complete);
        assert_eq!(report.imports[0].status, Ternary::Unknown);
    }
}
