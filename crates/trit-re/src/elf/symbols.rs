//! 符号表读取：`.dynsym` / `.symtab`。

use super::{DynamicSymbol, ElfSection, FunctionSymbol, cstr, u16_at, u32_at, u64_at};

const MAX_SYMBOLS: usize = 8192;
const SHT_DYNSYM: u32 = 11;

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

#[cfg(test)]
mod tests {
    #[test]
    fn imports_exclude_object_symbols() {
        use super::super::{ElfSection, parse_imports};
        // .dynstr: "\0socket\0environ\0"；.dynsym: 0 号空、1 号 FUNC、2 号 OBJECT。
        let mut bytes = vec![0u8; 0x80];
        bytes[1..8].copy_from_slice(b"socket\0");
        bytes[8..16].copy_from_slice(b"environ\0");
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
        use super::super::ElfSection;
        use super::read_function_symbols;
        // .strtab: "\0foo\0"；.symtab: 1 个 STT_FUNC（0x400100 + 0x200）。
        let mut bytes = vec![0u8; 0x40];
        bytes[0] = 0;
        bytes[1..5].copy_from_slice(b"foo\0");
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
}
