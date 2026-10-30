//! `.eh_frame` FDE 解析：裁剪 ELF 的精确函数边界。

use super::{ElfSection, FunctionSymbol, u32_at, u64_at};

/// 解析 `.eh_frame` 里的 FDE，得到带精确大小的函数区间。
///
/// 只支持常见的 FDE 编码（`pcrel|sdata4`、`udata4`、`absptr`）；其余编码跳过而不是猜。
/// 裁剪过的 ELF（无 `.symtab`）靠这个拿函数边界。
pub fn read_eh_frame(
    bytes: &[u8],
    sections: &[ElfSection],
    sixty_four: bool,
) -> Vec<FunctionSymbol> {
    let Some(section) = sections.iter().find(|section| section.name == ".eh_frame") else {
        return Vec::new();
    };
    let start = section.offset as usize;
    let end = start
        .checked_add(section.size as usize)
        .and_then(|end| bytes.get(start..end).map(|data| (data, end)));
    let Some((data, _)) = end else {
        return Vec::new();
    };
    let mut cies: std::collections::BTreeMap<usize, u8> = std::collections::BTreeMap::new();
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
        if cie_id == 0
            && let Some(encoding) = parse_eh_cie(data, id_pos + id_size, sixty_four)
        {
            cies.insert(entry_start, encoding);
        } else {
            let cie_start = (id_pos as u64).wrapping_sub(cie_id) as usize;
            if let Some(&encoding) = cies.get(&cie_start)
                && let Some((begin, range)) =
                    parse_eh_fde(data, id_pos + id_size, encoding, section.addr, sixty_four)
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

fn read_uleb(data: &[u8], cursor: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(*cursor)?;
        *cursor += 1;
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn read_sleb(data: &[u8], cursor: &mut usize) -> Option<i64> {
    let mut result = 0i64;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(*cursor)?;
        *cursor += 1;
        result |= ((byte & 0x7f) as i64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 64 && byte & 0x40 != 0 {
                result |= -1i64 << shift;
            }
            return Some(result);
        }
        if shift >= 64 {
            return None;
        }
    }
}

fn parse_eh_cie(data: &[u8], mut cursor: usize, sixty_four: bool) -> Option<u8> {
    let version = *data.get(cursor)?;
    cursor += 1;
    let aug_start = cursor;
    let aug_end = data.get(cursor..)?.iter().position(|byte| *byte == 0)? + cursor;
    let augmentation = data.get(aug_start..aug_end)?;
    cursor = aug_end + 1;
    let _code_align = read_uleb(data, &mut cursor)?;
    let _data_align = read_sleb(data, &mut cursor)?;
    if version == 1 {
        cursor += 1;
    } else {
        let _return_register = read_uleb(data, &mut cursor)?;
    }
    if !augmentation.starts_with(b"z") {
        return None;
    }
    let aug_len = read_uleb(data, &mut cursor)? as usize;
    let aug_end = cursor.checked_add(aug_len)?.min(data.len());
    let mut position = cursor;
    let mut encoding = None;
    for tag in &augmentation[1..] {
        match *tag {
            b'R' => {
                encoding = data.get(position).copied();
                position += 1;
            }
            b'L' => position += 1,
            b'P' => {
                let pointer_encoding = *data.get(position)?;
                position += 1;
                position += encoded_pointer_size(pointer_encoding, sixty_four)?;
            }
            b'S' | b'B' | b'G' => {}
            _ => return None,
        }
        if position > aug_end {
            return None;
        }
    }
    encoding
}

fn encoded_pointer_size(encoding: u8, sixty_four: bool) -> Option<usize> {
    match encoding {
        0x01 => Some(if sixty_four { 8 } else { 4 }),
        0x02 | 0x0a | 0x0b | 0x03 | 0x1b => Some(4),
        0x04 | 0x0c => Some(8),
        _ => None,
    }
}

fn parse_eh_fde(
    data: &[u8],
    cursor: usize,
    encoding: u8,
    section_addr: u64,
    sixty_four: bool,
) -> Option<(u64, u64)> {
    let mut position = cursor;
    let field_addr = section_addr + position as u64;
    let begin = match encoding {
        0x1b => {
            let relative = i32::from_le_bytes(data.get(position..position + 4)?.try_into().ok()?);
            position += 4;
            field_addr.wrapping_add(relative as i64 as u64)
        }
        0x03 => {
            let absolute = u32::from_le_bytes(data.get(position..position + 4)?.try_into().ok()?);
            position += 4;
            absolute as u64
        }
        0x0b => {
            let absolute = i32::from_le_bytes(data.get(position..position + 4)?.try_into().ok()?);
            position += 4;
            absolute as u64
        }
        0x01 => {
            if sixty_four {
                let absolute =
                    u64::from_le_bytes(data.get(position..position + 8)?.try_into().ok()?);
                position += 8;
                absolute
            } else {
                let absolute =
                    u32::from_le_bytes(data.get(position..position + 4)?.try_into().ok()?);
                position += 4;
                absolute as u64
            }
        }
        _ => return None,
    };
    let range = match encoded_pointer_size(encoding, sixty_four)? {
        8 => u64::from_le_bytes(data.get(position..position + 8)?.try_into().ok()?),
        _ => u32::from_le_bytes(data.get(position..position + 4)?.try_into().ok()?) as u64,
    };
    if begin == 0 || range == 0 {
        return None;
    }
    Some((begin, range))
}

#[cfg(test)]
mod tests {
    #[test]
    fn eh_frame_fde_gives_exact_range() {
        use super::{ElfSection, read_eh_frame};
        let mut bytes = vec![0u8; 0x40];
        let put32 = |bytes: &mut [u8], off: usize, value: u32| {
            bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
        };
        let put16 = |bytes: &mut [u8], off: usize, value: u16| {
            bytes[off..off + 2].copy_from_slice(&value.to_le_bytes());
        };
        let put8 = |bytes: &mut [u8], off: usize, value: u8| {
            bytes[off] = value;
        };
        // CIE：length=13，id=0，version=1，aug "zR"，code=1，data=-8，ret=30，aug_len=1，enc=0x1b。
        put32(&mut bytes, 0, 13);
        put32(&mut bytes, 4, 0);
        put8(&mut bytes, 8, 1);
        put8(&mut bytes, 9, b'z');
        put8(&mut bytes, 10, b'R');
        put8(&mut bytes, 11, 0);
        put8(&mut bytes, 12, 1);
        put8(&mut bytes, 13, 0x78);
        put8(&mut bytes, 14, 30);
        put8(&mut bytes, 15, 1);
        put8(&mut bytes, 16, 0x1b);
        // FDE：length=12，CIE 指针指向 CIE 起点（21），pcrel sdata4，range=0x40。
        put32(&mut bytes, 17, 12);
        put32(&mut bytes, 21, 21);
        put32(&mut bytes, 25, 0xe7);
        put32(&mut bytes, 29, 0x40);
        put16(&mut bytes, 33, 0);
        let sections = vec![ElfSection {
            name: ".eh_frame".into(),
            sh_type: 1,
            flags: 2,
            addr: 0x1000,
            offset: 0,
            size: 0x30,
            link: 0,
            entsize: 0,
            entropy: None,
        }];
        let ranges = read_eh_frame(&bytes, &sections, true);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0x1100);
        assert_eq!(ranges[0].size, 0x40);
    }

    #[test]
    fn eh_frame_cie_only_has_no_ranges() {
        use super::{ElfSection, read_eh_frame};
        let mut bytes = vec![0u8; 0x20];
        bytes[0..4].copy_from_slice(&13u32.to_le_bytes());
        bytes[9] = b'z';
        bytes[10] = b'R';
        bytes[16] = 0x1b;
        let sections = vec![ElfSection {
            name: ".eh_frame".into(),
            sh_type: 1,
            flags: 2,
            addr: 0x1000,
            offset: 0,
            size: 0x20,
            link: 0,
            entsize: 0,
            entropy: None,
        }];
        assert!(read_eh_frame(&bytes, &sections, true).is_empty());
    }
}
