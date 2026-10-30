//! `.debug_frame`（DWARF CFI）解析：与 `.eh_frame` 互补的函数边界来源。
//!
//! 两者的容器格式相似但语义不同：
//! - `.debug_frame` 的 CIE 标记是全 1（DWARF32 `0xffffffff` / DWARF64 `u64::MAX`），
//!   FDE 的 CIE 指针是**节内偏移**（不是回退距离）；
//! - FDE 的 `initial_location` / `address_range` 是**绝对地址**，宽度由 CIE 的
//!   `address_size` 决定（v4+ 显式给出，老版本按 ELF class 推断）。
//!
//! 只支持空 augmentation 的 CIE；带 augmentation 的条目跳过而不是猜。

use std::collections::BTreeMap;

use super::{ElfSection, FunctionSymbol, read_sleb, read_uleb, u32_at, u64_at};

/// 解析 `.debug_frame` 的 FDE，得到带精确大小的函数区间。
pub fn read_debug_frame(
    bytes: &[u8],
    sections: &[ElfSection],
    sixty_four: bool,
) -> Vec<FunctionSymbol> {
    let Some(section) = sections
        .iter()
        .find(|section| section.name == ".debug_frame")
    else {
        return Vec::new();
    };
    let start = section.offset as usize;
    let Some(data) = start
        .checked_add(section.size as usize)
        .and_then(|end| bytes.get(start..end))
    else {
        return Vec::new();
    };
    let mut cies: BTreeMap<usize, usize> = BTreeMap::new();
    let mut symbols = Vec::new();
    let mut pos = 0usize;
    while pos + 4 <= data.len() {
        let entry_start = pos;
        let mut cursor = pos;
        let Some(length32) = u32_at(data, cursor) else {
            break;
        };
        cursor += 4;
        let (length, dwarf64) = if length32 == 0xffff_ffff {
            let Some(length) = u64_at(data, cursor) else {
                break;
            };
            cursor += 8;
            (length as usize, true)
        } else {
            (length32 as usize, false)
        };
        if length == 0 {
            break;
        }
        let Some(body_end) = cursor.checked_add(length) else {
            break;
        };
        if body_end > data.len() {
            break;
        }
        let id_size = if dwarf64 { 8 } else { 4 };
        let id_pos = cursor;
        let Some(cie_id) = (if dwarf64 {
            u64_at(data, id_pos)
        } else {
            u32_at(data, id_pos).map(|value| value as u64)
        }) else {
            break;
        };
        let cie_marker = if dwarf64 { u64::MAX } else { 0xffff_ffff };
        if cie_id == cie_marker
            && let Some(address_size) = parse_cie(data, id_pos + id_size, sixty_four)
        {
            cies.insert(entry_start, address_size);
        } else {
            // `.debug_frame` 的 CIE 指针是节内偏移，不是回退距离。
            if let Some(&address_size) = cies.get(&(cie_id as usize))
                && let Some((begin, range)) = parse_fde(data, id_pos + id_size, address_size)
            {
                symbols.push(FunctionSymbol {
                    start: begin,
                    size: range,
                    name: String::new(),
                });
            }
        }
        pos = body_end;
    }
    symbols.sort_by_key(|symbol| (symbol.start, symbol.size));
    symbols.dedup_by(|left, right| left.start == right.start && left.size == right.size);
    symbols
}

fn parse_cie(data: &[u8], mut cursor: usize, sixty_four: bool) -> Option<usize> {
    let version = *data.get(cursor)?;
    cursor += 1;
    let aug_end = data.get(cursor..)?.iter().position(|byte| *byte == 0)? + cursor;
    let augmentation = data.get(cursor..aug_end)?;
    cursor = aug_end + 1;
    // 带 augmentation 的 CIE 需要解释额外字段；这里不猜。
    if !augmentation.is_empty() {
        return None;
    }
    let _code_alignment = read_uleb(data, &mut cursor)?;
    let _data_alignment = read_sleb(data, &mut cursor)?;
    if version == 1 {
        cursor += 1;
    } else {
        let _return_register = read_uleb(data, &mut cursor)?;
    }
    let mut address_size = if sixty_four { 8 } else { 4 };
    if version >= 4 {
        address_size = *data.get(cursor)? as usize;
        cursor += 1;
        let segment_size = *data.get(cursor)?;
        if segment_size != 0 {
            return None;
        }
    }
    if matches!(address_size, 4 | 8) {
        Some(address_size)
    } else {
        None
    }
}

fn parse_fde(data: &[u8], mut cursor: usize, address_size: usize) -> Option<(u64, u64)> {
    let begin = read_address(data, &mut cursor, address_size)?;
    let range = read_address(data, &mut cursor, address_size)?;
    if begin == 0 || range == 0 {
        return None;
    }
    Some((begin, range))
}

fn read_address(data: &[u8], cursor: &mut usize, address_size: usize) -> Option<u64> {
    match address_size {
        4 => {
            let value = u32_at(data, *cursor)? as u64;
            *cursor += 4;
            Some(value)
        }
        8 => {
            let value = u64_at(data, *cursor)?;
            *cursor += 8;
            Some(value)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ElfSection, read_debug_frame};

    #[test]
    fn debug_frame_fde_gives_exact_range() {
        let mut bytes = vec![0u8; 0x40];
        let put32 = |bytes: &mut [u8], off: usize, value: u32| {
            bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
        };
        let put64 = |bytes: &mut [u8], off: usize, value: u64| {
            bytes[off..off + 8].copy_from_slice(&value.to_le_bytes());
        };
        // CIE（v4，空 augmentation）：length=11，CIE 标记=0xffffffff。
        put32(&mut bytes, 0, 11);
        put32(&mut bytes, 4, 0xffff_ffff);
        bytes[8] = 4; // version
        bytes[9] = 0; // augmentation ""
        bytes[10] = 1; // code alignment
        bytes[11] = 0x78; // data alignment -8
        bytes[12] = 30; // return register
        bytes[13] = 8; // address_size
        bytes[14] = 0; // segment_selector_size
        // FDE：length=20，CIE 指针=0（节内偏移），绝对地址 0x1100，范围 0x40。
        put32(&mut bytes, 15, 20);
        put32(&mut bytes, 19, 0);
        put64(&mut bytes, 23, 0x1100);
        put64(&mut bytes, 31, 0x40);
        let sections = vec![ElfSection {
            name: ".debug_frame".into(),
            sh_type: 1,
            flags: 0,
            addr: 0,
            offset: 0,
            size: 0x40,
            link: 0,
            entsize: 0,
            entropy: None,
        }];
        let ranges = read_debug_frame(&bytes, &sections, true);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0x1100);
        assert_eq!(ranges[0].size, 0x40);
    }

    #[test]
    fn augmentation_is_skipped_not_guessed() {
        let mut bytes = vec![0u8; 0x20];
        bytes[0..4].copy_from_slice(&11u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        bytes[8] = 4;
        bytes[9] = b'z';
        let sections = vec![ElfSection {
            name: ".debug_frame".into(),
            sh_type: 1,
            flags: 0,
            addr: 0,
            offset: 0,
            size: 0x20,
            link: 0,
            entsize: 0,
            entropy: None,
        }];
        assert!(read_debug_frame(&bytes, &sections, true).is_empty());
    }
}
