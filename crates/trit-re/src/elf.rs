//! ELF32/ELF64 的最小解析（小端）。
//!
//! 只读逆向需要的结构：节表、程序头、动态导入、`.comment` 编译器指纹、
//! GNU build-id、解释器、叠加段。大端样本直接报"暂不支持"，不装作解析成功。

use std::fmt;

use crate::entropy;

/// ELF 解析失败原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError(pub &'static str);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ParseError {}

const MAX_PHNUM: usize = 256;
const MAX_SHNUM: usize = 4096;
const MAX_SYMBOLS: usize = 8192;

const SHT_PROGBITS: u32 = 1;
const SHT_DYNSYM: u32 = 11;
const SHT_NOTE: u32 = 7;
const SHT_NOBITS: u32 = 8;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const SHF_WRITE: u64 = 0x1;
const SHF_EXECINSTR: u64 = 0x4;

/// 程序头。
#[derive(Debug, Clone)]
pub struct Phdr {
    /// 段类型。
    pub p_type: u32,
    /// 段标志（R=4/W=2/X=1）。
    pub flags: u32,
    /// 文件偏移。
    pub offset: u64,
    /// 虚拟地址。
    pub vaddr: u64,
    /// 文件内大小。
    pub filesz: u64,
    /// 内存内大小。
    pub memsz: u64,
}

impl Phdr {
    /// 是否可执行。
    pub fn executable(&self) -> bool {
        self.flags & PF_X != 0
    }

    /// 是否可写。
    pub fn writable(&self) -> bool {
        self.flags & 2 != 0
    }
}

/// 节。
#[derive(Debug, Clone)]
pub struct ElfSection {
    /// 节名。
    pub name: String,
    /// 节类型。
    pub sh_type: u32,
    /// 节标志。
    pub flags: u64,
    /// 虚拟地址。
    pub addr: u64,
    /// 文件偏移。
    pub offset: u64,
    /// 大小。
    pub size: u64,
    /// 关联节下标。
    pub link: u32,
    /// 条目大小。
    pub entsize: u64,
    /// 原始数据熵（`NOBITS`/越界时为 `None`）。
    pub entropy: Option<f64>,
}

/// 动态符号表里的一条（`xref` 需要下标来解重定位）。
#[derive(Debug, Clone)]
pub struct DynamicSymbol {
    /// 符号下标。
    pub index: usize,
    /// 符号名。
    pub name: String,
    /// 是否有定义（`st_shndx != 0`）。
    pub defined: bool,
    /// 符号类型（`st_info & 0xf`）：2=FUNC、0=NOTYPE、1=OBJECT…
    pub kind: u8,
}

/// 带大小的函数符号（`st_size > 0`，可当精确函数边界）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSymbol {
    /// 起始虚拟地址。
    pub start: u64,
    /// 字节大小。
    pub size: u64,
    /// 符号名。
    pub name: String,
}

/// 叠加段。
#[derive(Debug, Clone)]
pub struct Overlay {
    /// 起始偏移。
    pub offset: usize,
    /// 字节数。
    pub size: usize,
    /// 熵。
    pub entropy: f64,
}

/// ELF 结构摘要。
#[derive(Debug, Clone)]
pub struct ElfInfo {
    /// 1=ELF32，2=ELF64。
    pub class: u8,
    /// 文件类型（ET_EXEC/ET_DYN/ET_REL…）。
    pub etype: u16,
    /// 机器码。
    pub machine: u16,
    /// 入口地址。
    pub entry: u64,
    /// 程序头。
    pub phdrs: Vec<Phdr>,
    /// 节表。
    pub sections: Vec<ElfSection>,
    /// 动态导入；`None` 表示节表缺失或不可信。
    pub imports: Option<Vec<String>>,
    /// `.comment` 里的编译器指纹。
    pub comment: Option<String>,
    /// GNU build-id（十六进制）。
    pub build_id: Option<String>,
    /// `.interp`（动态加载器路径）。
    pub interp: Option<String>,
    /// 叠加段。
    pub overlay: Option<Overlay>,
    /// 是否有节表。
    pub has_section_headers: bool,
}

/// 解析 ELF；非 ELF、大端或结构损坏时返回错误。
pub fn parse(bytes: &[u8]) -> Result<ElfInfo, ParseError> {
    if bytes.len() < 0x34 || &bytes[..4] != b"\x7fELF" {
        return Err(ParseError("不是 ELF 魔数"));
    }
    let class = bytes[4];
    if !matches!(class, 1 | 2) {
        return Err(ParseError("未知 ELF class"));
    }
    let data = bytes[5];
    if data != 1 {
        return Err(ParseError("大端 ELF 暂不支持"));
    }
    let sixty_four = class == 2;
    let etype = u16_at(bytes, 16).ok_or(ParseError("ELF 头截断"))?;
    let machine = u16_at(bytes, 18).ok_or(ParseError("ELF 头截断"))?;
    let (entry, phoff, shoff) = if sixty_four {
        (
            u64_at(bytes, 24).ok_or(ParseError("ELF 头截断"))?,
            u64_at(bytes, 32).ok_or(ParseError("ELF 头截断"))?,
            u64_at(bytes, 40).ok_or(ParseError("ELF 头截断"))?,
        )
    } else {
        (
            u32_at(bytes, 24).ok_or(ParseError("ELF 头截断"))? as u64,
            u32_at(bytes, 28).ok_or(ParseError("ELF 头截断"))? as u64,
            u32_at(bytes, 32).ok_or(ParseError("ELF 头截断"))? as u64,
        )
    };
    let (phentsize, phnum, shentsize, shnum_raw, shstrndx) = if sixty_four {
        (
            u16_at(bytes, 54).ok_or(ParseError("ELF 头截断"))?,
            u16_at(bytes, 56).ok_or(ParseError("ELF 头截断"))?,
            u16_at(bytes, 58).ok_or(ParseError("ELF 头截断"))?,
            u16_at(bytes, 60).ok_or(ParseError("ELF 头截断"))?,
            u16_at(bytes, 62).ok_or(ParseError("ELF 头截断"))?,
        )
    } else {
        (
            u16_at(bytes, 42).ok_or(ParseError("ELF 头截断"))?,
            u16_at(bytes, 44).ok_or(ParseError("ELF 头截断"))?,
            u16_at(bytes, 46).ok_or(ParseError("ELF 头截断"))?,
            u16_at(bytes, 48).ok_or(ParseError("ELF 头截断"))?,
            u16_at(bytes, 50).ok_or(ParseError("ELF 头截断"))?,
        )
    };

    let mut phdrs = Vec::new();
    let phdr_size = if sixty_four { 56 } else { 32 };
    if phoff != 0 && phnum != 0 {
        let step = if phentsize as usize >= phdr_size {
            phentsize as usize
        } else {
            phdr_size
        };
        for index in 0..(phnum as usize).min(MAX_PHNUM) {
            let base = phoff as usize + index * step;
            let phdr = if sixty_four {
                Phdr {
                    p_type: u32_at(bytes, base).ok_or(ParseError("程序头截断"))?,
                    flags: u32_at(bytes, base + 4).ok_or(ParseError("程序头截断"))?,
                    offset: u64_at(bytes, base + 8).ok_or(ParseError("程序头截断"))?,
                    vaddr: u64_at(bytes, base + 16).ok_or(ParseError("程序头截断"))?,
                    filesz: u64_at(bytes, base + 32).ok_or(ParseError("程序头截断"))?,
                    memsz: u64_at(bytes, base + 40).ok_or(ParseError("程序头截断"))?,
                }
            } else {
                Phdr {
                    p_type: u32_at(bytes, base).ok_or(ParseError("程序头截断"))?,
                    offset: u32_at(bytes, base + 4).ok_or(ParseError("程序头截断"))? as u64,
                    vaddr: u32_at(bytes, base + 8).ok_or(ParseError("程序头截断"))? as u64,
                    filesz: u32_at(bytes, base + 16).ok_or(ParseError("程序头截断"))? as u64,
                    memsz: u32_at(bytes, base + 20).ok_or(ParseError("程序头截断"))? as u64,
                    flags: u32_at(bytes, base + 24).ok_or(ParseError("程序头截断"))?,
                }
            };
            phdrs.push(phdr);
        }
    }

    let shdr_size = if sixty_four { 64 } else { 40 };
    let step = if shentsize as usize >= shdr_size {
        shentsize as usize
    } else {
        shdr_size
    };
    let shnum = if shnum_raw == 0 && shoff != 0 {
        // 扩展节号：真实数量在 0 号节的 sh_size 里。
        let base = shoff as usize;
        if sixty_four {
            u64_at(bytes, base + 32).ok_or(ParseError("节表截断"))? as usize
        } else {
            u32_at(bytes, base + 20).ok_or(ParseError("节表截断"))? as usize
        }
    } else {
        shnum_raw as usize
    };
    let shnum = shnum.min(MAX_SHNUM);
    let has_section_headers = shoff != 0 && shnum > 0;

    struct RawShdr {
        name_off: u32,
        sh_type: u32,
        flags: u64,
        addr: u64,
        offset: u64,
        size: u64,
        link: u32,
        entsize: u64,
    }

    let mut raw_shdrs = Vec::new();
    for index in 0..shnum {
        let base = shoff as usize + index * step;
        let raw = if sixty_four {
            RawShdr {
                name_off: u32_at(bytes, base).ok_or(ParseError("节表截断"))?,
                sh_type: u32_at(bytes, base + 4).ok_or(ParseError("节表截断"))?,
                flags: u64_at(bytes, base + 8).ok_or(ParseError("节表截断"))?,
                addr: u64_at(bytes, base + 16).ok_or(ParseError("节表截断"))?,
                offset: u64_at(bytes, base + 24).ok_or(ParseError("节表截断"))?,
                size: u64_at(bytes, base + 32).ok_or(ParseError("节表截断"))?,
                link: u32_at(bytes, base + 40).ok_or(ParseError("节表截断"))?,
                entsize: u64_at(bytes, base + 56).ok_or(ParseError("节表截断"))?,
            }
        } else {
            RawShdr {
                name_off: u32_at(bytes, base).ok_or(ParseError("节表截断"))?,
                sh_type: u32_at(bytes, base + 4).ok_or(ParseError("节表截断"))?,
                flags: u32_at(bytes, base + 8).ok_or(ParseError("节表截断"))? as u64,
                addr: u32_at(bytes, base + 12).ok_or(ParseError("节表截断"))? as u64,
                offset: u32_at(bytes, base + 16).ok_or(ParseError("节表截断"))? as u64,
                size: u32_at(bytes, base + 20).ok_or(ParseError("节表截断"))? as u64,
                link: u32_at(bytes, base + 24).ok_or(ParseError("节表截断"))?,
                entsize: u32_at(bytes, base + 36).ok_or(ParseError("节表截断"))? as u64,
            }
        };
        raw_shdrs.push(raw);
    }

    let strtab_index = if shstrndx == 0xffff {
        raw_shdrs.first().map_or(0, |header| header.link as usize)
    } else {
        shstrndx as usize
    };
    let shstr = raw_shdrs.get(strtab_index).and_then(|header| {
        let start = header.offset as usize;
        let end = start.checked_add(header.size as usize)?;
        bytes.get(start..end)
    });

    let mut sections = Vec::with_capacity(raw_shdrs.len());
    for raw in &raw_shdrs {
        let name = shstr
            .and_then(|table| cstr(table, raw.name_off as usize, 256))
            .unwrap_or_default();
        let data = if raw.sh_type == SHT_NOBITS || raw.size == 0 {
            None
        } else {
            let start = raw.offset as usize;
            start
                .checked_add(raw.size as usize)
                .and_then(|end| bytes.get(start..end))
        };
        sections.push(ElfSection {
            name,
            sh_type: raw.sh_type,
            flags: raw.flags,
            addr: raw.addr,
            offset: raw.offset,
            size: raw.size,
            link: raw.link,
            entsize: raw.entsize,
            entropy: data.map(entropy::shannon),
        });
    }

    let imports = parse_imports(bytes, &sections, sixty_four);
    let comment = sections
        .iter()
        .find(|section| section.name == ".comment" && section.sh_type == SHT_PROGBITS)
        .and_then(|section| {
            let data = bytes
                .get(section.offset as usize..section.offset as usize + section.size as usize)?;
            let text: Vec<String> = data
                .split(|b| *b == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).trim().to_string())
                .filter(|part| !part.is_empty())
                .collect();
            if text.is_empty() {
                None
            } else {
                Some(text.join("; "))
            }
        });
    let build_id = sections
        .iter()
        .filter(|section| section.sh_type == SHT_NOTE)
        .find_map(|section| note_build_id(bytes, section));
    let interp = sections
        .iter()
        .find(|section| section.name == ".interp")
        .and_then(|section| cstr(bytes.get(section.offset as usize..)?, 0, 256));

    let mut end = 0usize;
    for phdr in &phdrs {
        if phdr.p_type == PT_LOAD {
            end = end.max((phdr.offset as usize).saturating_add(phdr.filesz as usize));
        }
    }
    for section in &sections {
        if section.sh_type != SHT_NOBITS {
            end = end.max((section.offset as usize).saturating_add(section.size as usize));
        }
    }
    // 节头表本身不算叠加段：它的内容可能排在最后一个节之后。
    if shoff != 0 {
        end = end.max((shoff as usize).saturating_add(shnum.saturating_mul(step)));
    }
    let overlay = if bytes.len() > end && end >= 0x40 {
        let data = &bytes[end..];
        Some(Overlay {
            offset: end,
            size: data.len(),
            entropy: entropy::shannon(data),
        })
    } else {
        None
    };

    Ok(ElfInfo {
        class,
        etype,
        machine,
        entry,
        phdrs,
        sections,
        imports,
        comment,
        build_id,
        interp,
        overlay,
        has_section_headers,
    })
}

fn parse_imports(bytes: &[u8], sections: &[ElfSection], sixty_four: bool) -> Option<Vec<String>> {
    let symbols = read_dynamic_symbols(bytes, sections, sixty_four)?;
    Some(
        // 只把函数类符号当"导入"：OBJECT（如 environ/__stack_chk_guard）是数据，
        // 拿它问"有没有调用点"没有意义。
        symbols
            .into_iter()
            .filter(|symbol| !symbol.defined && matches!(symbol.kind, 0 | 2 | 10))
            .map(|symbol| symbol.name)
            .collect(),
    )
}

/// 读取全部动态符号（含下标）；没有节表时返回 `None`。
pub fn read_dynamic_symbols(
    bytes: &[u8],
    sections: &[ElfSection],
    sixty_four: bool,
) -> Option<Vec<DynamicSymbol>> {
    let Some(dynsym) = sections
        .iter()
        .find(|section| section.sh_type == SHT_DYNSYM)
    else {
        // 没有节表（常见于删节表加壳）→ 不知道有哪些导入。
        return if sections.is_empty() {
            None
        } else {
            Some(Vec::new())
        };
    };
    let strtab = sections.get(dynsym.link as usize)?;
    let strings =
        bytes.get(strtab.offset as usize..strtab.offset as usize + strtab.size as usize)?;
    let entry_size = if dynsym.entsize > 0 {
        dynsym.entsize as usize
    } else if sixty_four {
        24
    } else {
        16
    };
    if entry_size < if sixty_four { 24 } else { 16 } {
        return None;
    }
    let count = (dynsym.size as usize / entry_size).min(MAX_SYMBOLS);
    let base = dynsym.offset as usize;
    let mut symbols = Vec::new();
    for index in 0..count {
        let off = base + index * entry_size;
        let name_off = u32_at(bytes, off)? as usize;
        let info = bytes.get(off + if sixty_four { 4 } else { 12 }).copied()?;
        let shndx = if sixty_four {
            u16_at(bytes, off + 6)?
        } else {
            u16_at(bytes, off + 14)?
        };
        if let Some(name) = cstr(strings, name_off, 512) {
            symbols.push(DynamicSymbol {
                index,
                name,
                defined: shndx != 0,
                kind: info & 0xf,
            });
        }
    }
    Some(symbols)
}

/// 读取 SYMTAB / DYNSYM 里 `STT_FUNC` 且 `st_size > 0` 的符号。
pub fn read_function_symbols(
    bytes: &[u8],
    sections: &[ElfSection],
    sixty_four: bool,
) -> Vec<FunctionSymbol> {
    let mut symbols = Vec::new();
    for section in sections {
        if section.sh_type != 2 && section.sh_type != SHT_DYNSYM {
            continue;
        }
        let Some(strtab) = sections.get(section.link as usize) else {
            continue;
        };
        let Some(strings) = bytes.get(strtab.offset as usize..) else {
            continue;
        };
        let default_size = if sixty_four { 24 } else { 16 };
        let entry_size = if section.entsize > 0 {
            section.entsize as usize
        } else {
            default_size
        };
        if entry_size < default_size {
            continue;
        }
        let count = (section.size as usize / entry_size).min(MAX_SYMBOLS);
        for index in 0..count {
            let base = section.offset as usize + index * entry_size;
            let Some(info) = bytes.get(base + if sixty_four { 4 } else { 12 }).copied() else {
                break;
            };
            if info & 0xf != 2 {
                continue;
            }
            let shndx = if sixty_four {
                u16_at(bytes, base + 6)
            } else {
                u16_at(bytes, base + 14)
            };
            let Some(shndx) = shndx else {
                break;
            };
            if shndx == 0 {
                continue;
            }
            let (value, size) = if sixty_four {
                (u64_at(bytes, base + 8), u64_at(bytes, base + 16))
            } else {
                (
                    u32_at(bytes, base + 4).map(|value| value as u64),
                    u32_at(bytes, base + 8).map(|value| value as u64),
                )
            };
            let (Some(value), Some(size)) = (value, size) else {
                break;
            };
            if value == 0 || size == 0 {
                continue;
            }
            let name_off = u32_at(bytes, base)
                .map(|offset| offset as usize)
                .unwrap_or(0);
            let name = cstr(strings, name_off, 512).unwrap_or_default();
            symbols.push(FunctionSymbol {
                start: value,
                size,
                name,
            });
        }
    }
    symbols.sort_by_key(|symbol| (symbol.start, symbol.size));
    symbols.dedup_by(|left, right| left.start == right.start && left.size == right.size);
    symbols
}

fn note_build_id(bytes: &[u8], section: &ElfSection) -> Option<String> {
    let data =
        bytes.get(section.offset as usize..section.offset as usize + section.size as usize)?;
    let mut cursor = 0usize;
    while cursor + 12 <= data.len() {
        let namesz = u32_at(data, cursor)? as usize;
        let descsz = u32_at(data, cursor + 4)? as usize;
        let note_type = u32_at(data, cursor + 8)?;
        let name_start = cursor + 12;
        let name = data.get(name_start..name_start + namesz)?;
        let desc_start = name_start + namesz.div_ceil(4) * 4;
        let desc = data.get(desc_start..desc_start + descsz)?;
        if note_type == 3 && name.starts_with(b"GNU") && !desc.is_empty() {
            let mut hex = String::with_capacity(desc.len() * 2);
            for byte in desc.iter().take(64) {
                hex.push_str(&format!("{byte:02x}"));
            }
            return Some(hex);
        }
        cursor = desc_start + descsz.div_ceil(4) * 4;
    }
    None
}

impl ElfInfo {
    /// 架构短名。
    pub fn arch(&self) -> &'static str {
        machine_name(self.machine)
    }

    /// 格式短名。
    pub fn format(&self) -> &'static str {
        if self.class == 2 { "elf64" } else { "elf32" }
    }

    /// 入口是否落在可执行段内；没有程序头时返回 `None`。
    pub fn entry_in_executable(&self) -> Option<bool> {
        if self.phdrs.is_empty() {
            return None;
        }
        if self.entry == 0 {
            return None;
        }
        Some(self.phdrs.iter().any(|phdr| {
            phdr.p_type == PT_LOAD
                && phdr.executable()
                && self.entry >= phdr.vaddr
                && self.entry < phdr.vaddr.saturating_add(phdr.memsz)
        }))
    }

    /// 读取带大小的函数符号（EXEC 段里的 STT_FUNC）。
    pub fn function_symbols(&self, bytes: &[u8]) -> Vec<FunctionSymbol> {
        read_function_symbols(bytes, &self.sections, self.class == 2)
    }

    /// 读取全部动态符号（供交叉引用解重定位用）。
    pub fn dynamic_symbols(&self, bytes: &[u8]) -> Option<Vec<DynamicSymbol>> {
        read_dynamic_symbols(bytes, &self.sections, self.class == 2)
    }

    /// 最大的可执行/可写段熵。
    pub fn max_section_entropy(&self) -> Option<f64> {
        self.sections
            .iter()
            .filter(|section| section.sh_type != SHT_NOBITS && section.size >= 4096)
            .filter(|section| section.flags & (SHF_WRITE | SHF_EXECINSTR) != 0)
            .filter_map(|section| section.entropy)
            .reduce(f64::max)
    }

    /// 是否有"可执行/可写但数据读不出"的节（熵未知）。
    pub fn has_unknown_entropy(&self) -> bool {
        self.sections.iter().any(|section| {
            section.sh_type != SHT_NOBITS
                && section.size >= 4096
                && section.flags & (SHF_WRITE | SHF_EXECINSTR) != 0
                && section.entropy.is_none()
        })
    }
}

/// e_machine → 架构名。
pub fn machine_name(machine: u16) -> &'static str {
    match machine {
        3 => "x86",
        62 => "x86-64",
        40 => "arm",
        183 => "arm64",
        8 => "mips",
        20 => "ppc",
        21 => "ppc64",
        243 => "riscv64",
        _ => "unknown",
    }
}

fn u16_at(bytes: &[u8], off: usize) -> Option<u16> {
    let slice = bytes.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], off: usize) -> Option<u32> {
    let slice = bytes.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn u64_at(bytes: &[u8], off: usize) -> Option<u64> {
    let slice = bytes.get(off..off.checked_add(8)?)?;
    let mut word = [0u8; 8];
    word.copy_from_slice(slice);
    Some(u64::from_le_bytes(word))
}

fn cstr(bytes: &[u8], off: usize, limit: usize) -> Option<String> {
    let slice = bytes.get(off..)?;
    let end = slice
        .iter()
        .take(limit)
        .position(|b| *b == 0)
        .unwrap_or(slice.len().min(limit));
    let text = &slice[..end];
    if text.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(text).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{ParseError, parse};

    #[test]
    fn rejects_non_elf() {
        assert_eq!(parse(b"not elf").unwrap_err(), ParseError("不是 ELF 魔数"));
    }

    #[test]
    fn rejects_big_endian_explicitly() {
        let mut bytes = vec![0u8; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 2;
        assert_eq!(parse(&bytes).unwrap_err(), ParseError("大端 ELF 暂不支持"));
    }

    #[test]
    fn imports_exclude_object_symbols() {
        use super::{ElfSection, parse_imports};
        // .dynstr: " socket environ "；.dynsym: 0 号空、1 号 FUNC、2 号 OBJECT。
        let mut bytes = vec![0u8; 0x80];
        bytes[1..8].copy_from_slice(b"socket ");
        bytes[8..16].copy_from_slice(b"environ ");
        let base = 0x40usize;
        let entry = |index: usize| base + index * 24;
        bytes[entry(1)..entry(1) + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[entry(1) + 4] = 0x12; // STT_FUNC | STB_GLOBAL
        bytes[entry(2)..entry(2) + 4].copy_from_slice(&8u32.to_le_bytes());
        bytes[entry(2) + 4] = 0x11; // STT_OBJECT | STB_GLOBAL
        let sections = vec![
            ElfSection {
                name: ".dynstr".into(),
                sh_type: 3,
                flags: 0,
                addr: 0,
                offset: 0,
                size: 0x10,
                link: 0,
                entsize: 0,
                entropy: None,
            },
            ElfSection {
                name: ".dynsym".into(),
                sh_type: 11,
                flags: 0,
                addr: 0,
                offset: base as u64,
                size: 72,
                link: 0,
                entsize: 24,
                entropy: None,
            },
        ];
        let imports = parse_imports(&bytes, &sections, true).expect("parsed");
        assert_eq!(imports, vec!["socket".to_string()]);
    }

    #[test]
    fn reads_sized_function_symbols() {
        use super::{ElfSection, read_function_symbols};
        // .strtab: " foo "；.symtab: 1 个 STT_FUNC（0x400100 + 0x200）。
        let mut bytes = vec![0u8; 0x40];
        bytes[0] = 0;
        bytes[1..5].copy_from_slice(b"foo ");
        let sym = 0x10usize;
        bytes[sym..sym + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[sym + 4] = 0x12; // STT_FUNC | STB_GLOBAL
        bytes[sym + 6..sym + 8].copy_from_slice(&1u16.to_le_bytes());
        bytes[sym + 8..sym + 16].copy_from_slice(&0x40_0100u64.to_le_bytes());
        bytes[sym + 16..sym + 24].copy_from_slice(&0x200u64.to_le_bytes());
        let sections = vec![
            ElfSection {
                name: ".strtab".into(),
                sh_type: 3,
                flags: 0,
                addr: 0,
                offset: 0,
                size: 5,
                link: 0,
                entsize: 0,
                entropy: None,
            },
            ElfSection {
                name: ".symtab".into(),
                sh_type: 2,
                flags: 0,
                addr: 0,
                offset: 0x10,
                size: 24,
                link: 0,
                entsize: 24,
                entropy: None,
            },
        ];
        let symbols = read_function_symbols(&bytes, &sections, true);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].start, 0x40_0100);
        assert_eq!(symbols[0].size, 0x200);
        assert_eq!(symbols[0].name, "foo");
    }

    #[test]
    fn truncated_header_is_an_error_not_a_panic() {
        let mut bytes = vec![0u8; 20];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        assert!(parse(&bytes).is_err());
    }
}
