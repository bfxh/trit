//! `.debug_frame`（DWARF CFI）解析：与 `.eh_frame` 互补的函数边界来源。
//!
//! 容器格式与 `.eh_frame` 相似但语义不同：
//! - CIE 标记是全 1（DWARF32 `0xffffffff` / DWARF64 `u64::MAX`），FDE 的 CIE 指针是**节内偏移**；
//! - FDE 的地址按 CIE 的 `address_size` 与指针编码（`zR`）解读；没有 `R` 时按绝对地址。
//!
//! 指针编码支持 `absptr` / `uleb128` / `sleb128` / `udata2/4/8` / `sdata2/4/8`，
//! 应用位支持绝对（`0x00`）与 `pcrel`（`0x10`）；`indirect`、`datarel` 等不认识就跳过。

use std::collections::BTreeMap;

use super::{ElfSection, FunctionSymbol, read_sleb, read_uleb, u32_at, u64_at};

/// CIE 里与 FDE 解码相关的信息。
#[derive(Debug, Clone, Copy)]
struct CieInfo {
    address_size: usize,
    /// FDE 指针编码（`0` = 绝对地址；`0xff` = 省略）。
    encoding: u8,
}

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
    let mut cies: BTreeMap<usize, CieInfo> = BTreeMap::new();
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
            && let Some(info) = parse_cie(data, id_pos + id_size, sixty_four)
        {
            cies.insert(entry_start, info);
        } else if let Some(&info) = cies.get(&(cie_id as usize))
            && let Some((begin, range)) = parse_fde(data, id_pos + id_size, info, section.addr)
        {
            symbols.push(FunctionSymbol {
                start: begin,
                size: range,
                name: String::new(),
            });
        }
        pos = body_end;
    }
    symbols.sort_by_key(|symbol| (symbol.start, symbol.size));
    symbols.dedup_by(|left, right| left.start == right.start && left.size == right.size);
    symbols
}

fn parse_cie(data: &[u8], mut cursor: usize, sixty_four: bool) -> Option<CieInfo> {
    let version = *data.get(cursor)?;
    cursor += 1;
    let aug_end = data.get(cursor..)?.iter().position(|byte| *byte == 0)? + cursor;
    let augmentation = data.get(cursor..aug_end)?;
    cursor = aug_end + 1;
    // DWARF v4：address_size / segment_selector_size 紧跟 augmentation，
    // 在 code_align 之前（binutils 也按这个顺序读；clang 的 .debug_frame 实测如此）。
    let mut address_size = if sixty_four { 8 } else { 4 };
    if version >= 4 {
        address_size = *data.get(cursor)? as usize;
        cursor += 1;
        let segment_size = *data.get(cursor)?;
        cursor += 1;
        if segment_size != 0 {
            return None;
        }
    }
    let _code_alignment = read_uleb(data, &mut cursor)?;
    let _data_alignment = read_sleb(data, &mut cursor)?;
    if version == 1 {
        cursor += 1;
    } else {
        let _return_register = read_uleb(data, &mut cursor)?;
    }
    if !matches!(address_size, 4 | 8) {
        return None;
    }
    let mut encoding = 0u8;
    if augmentation.is_empty() {
        // 没有 `z`：默认绝对地址。
    } else if augmentation.first() == Some(&b'z') {
        let aug_len = read_uleb(data, &mut cursor)? as usize;
        let aug_end = cursor.checked_add(aug_len)?;
        if aug_end > data.len() {
            return None;
        }
        let mut position = cursor;
        for tag in &augmentation[1..] {
            match *tag {
                b'R' => {
                    encoding = *data.get(position)?;
                    position += 1;
                }
                b'L' => position += 1,
                b'P' => {
                    let personality_encoding = *data.get(position)?;
                    position += 1;
                    skip_encoded(data, &mut position, personality_encoding, address_size)?;
                }
                b'S' | b'B' | b'G' => {}
                _ => return None,
            }
            if position > aug_end {
                return None;
            }
        }
    } else {
        return None;
    }
    if encoding != 0
        && encoding != 0xff
        && !matches!(encoding & 0x0f, 0 | 1 | 2 | 3 | 4 | 9 | 0xa | 0xb | 0xc)
    {
        return None;
    }
    Some(CieInfo {
        address_size,
        encoding,
    })
}

fn parse_fde(
    data: &[u8],
    mut cursor: usize,
    info: CieInfo,
    section_addr: u64,
) -> Option<(u64, u64)> {
    if info.encoding == 0xff {
        return None;
    }
    let field_addr = section_addr + cursor as u64;
    let (begin, _) = read_encoded(
        data,
        &mut cursor,
        info.encoding,
        info.address_size,
        field_addr,
    )?;
    // 范围字段与初址同格式，但按无符号整数读（不带 pcrel 等应用位）。
    let (range, _) = read_encoded(
        data,
        &mut cursor,
        info.encoding & 0x0f,
        info.address_size,
        0,
    )?;
    if begin == 0 || range == 0 {
        return None;
    }
    Some((begin, range))
}

fn read_encoded(
    data: &[u8],
    cursor: &mut usize,
    encoding: u8,
    address_size: usize,
    base: u64,
) -> Option<(u64, usize)> {
    if encoding == 0xff {
        return None;
    }
    let format = encoding & 0x0f;
    let (raw, size) = match format {
        0x00 => {
            let value = read_fixed(data, cursor, address_size)?;
            (value, address_size)
        }
        0x01 => (read_uleb(data, cursor)?, 0),
        0x02 => (
            u32::from(u16::from_le_bytes(read_array::<2>(data, cursor)?)) as u64,
            2,
        ),
        0x03 => (u32::from_le_bytes(read_array::<4>(data, cursor)?) as u64, 4),
        0x04 => (u64::from_le_bytes(read_array::<8>(data, cursor)?), 8),
        0x09 => (read_sleb(data, cursor)? as u64, 0),
        0x0a => (
            i16::from_le_bytes(read_array::<2>(data, cursor)?) as i64 as u64,
            2,
        ),
        0x0b => (
            i32::from_le_bytes(read_array::<4>(data, cursor)?) as i64 as u64,
            4,
        ),
        0x0c => (i64::from_le_bytes(read_array::<8>(data, cursor)?) as u64, 8),
        _ => return None,
    };
    let value = match encoding & 0x70 {
        0x00 => raw,
        0x10 => base.wrapping_add(raw),
        // textrel / datarel / funcrel / aligned：不认识，跳过而不是猜。
        _ => return None,
    };
    Some((value, size))
}

/// 只跳过编码后的指针（用于 CIE 的 `P` personality 指针）。
fn skip_encoded(data: &[u8], cursor: &mut usize, encoding: u8, address_size: usize) -> Option<()> {
    if encoding == 0xff {
        return Some(());
    }
    let format = encoding & 0x0f;
    match format {
        0x00 => {
            let _ = read_fixed(data, cursor, address_size)?;
        }
        0x01 => {
            let _ = read_uleb(data, cursor)?;
        }
        0x09 => {
            let _ = read_sleb(data, cursor)?;
        }
        0x02 | 0x0a => {
            let _ = read_array::<2>(data, cursor)?;
        }
        0x03 | 0x0b => {
            let _ = read_array::<4>(data, cursor)?;
        }
        0x04 | 0x0c => {
            let _ = read_array::<8>(data, cursor)?;
        }
        _ => return None,
    }
    Some(())
}

fn read_fixed(data: &[u8], cursor: &mut usize, size: usize) -> Option<u64> {
    match size {
        2 => Some(u16::from_le_bytes(read_array::<2>(data, cursor)?) as u64),
        4 => Some(u32::from_le_bytes(read_array::<4>(data, cursor)?) as u64),
        8 => Some(u64::from_le_bytes(read_array::<8>(data, cursor)?)),
        _ => None,
    }
}

fn read_array<const N: usize>(data: &[u8], cursor: &mut usize) -> Option<[u8; N]> {
    let slice = data.get(*cursor..cursor.checked_add(N)?)?;
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    *cursor += N;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{ElfSection, read_debug_frame};

    fn section(size: u64, addr: u64) -> ElfSection {
        ElfSection {
            name: ".debug_frame".into(),
            sh_type: 1,
            flags: 0,
            addr,
            offset: 0,
            size,
            link: 0,
            entsize: 0,
            entropy: None,
        }
    }

    fn put32(bytes: &mut [u8], off: usize, value: u32) {
        bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put64(bytes: &mut [u8], off: usize, value: u64) {
        bytes[off..off + 8].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn debug_frame_fde_gives_exact_range() {
        let mut bytes = vec![0u8; 0x40];
        // CIE（v4，空 augmentation）：length=11。
        put32(&mut bytes, 0, 11);
        put32(&mut bytes, 4, 0xffff_ffff);
        bytes[8] = 4;
        bytes[9] = 0;
        bytes[10] = 8;
        bytes[11] = 0;
        bytes[12] = 1;
        bytes[13] = 0x78;
        bytes[14] = 30;
        // FDE：length=20，CIE 指针 0，绝对地址 0x1100，范围 0x40。
        put32(&mut bytes, 15, 20);
        put32(&mut bytes, 19, 0);
        put64(&mut bytes, 23, 0x1100);
        put64(&mut bytes, 31, 0x40);
        let sections = vec![section(0x40, 0)];
        let ranges = read_debug_frame(&bytes, &sections, true);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0x1100);
        assert_eq!(ranges[0].size, 0x40);
    }

    #[test]
    fn address_size_eight_reads_full_width_addresses() {
        let mut bytes = vec![0u8; 0x30];
        // 地址 0x1_0000_1100 超出 32 位：写死 4 字节会读到低半截。
        put32(&mut bytes, 0, 11);
        put32(&mut bytes, 4, 0xffff_ffff);
        bytes[8] = 4;
        bytes[9] = 0;
        bytes[10] = 8;
        bytes[11] = 0;
        bytes[12] = 1;
        bytes[13] = 0x78;
        bytes[14] = 30;
        put32(&mut bytes, 15, 20);
        put32(&mut bytes, 19, 0);
        put64(&mut bytes, 23, 0x1_0000_1100);
        put64(&mut bytes, 31, 0x40);
        let sections = vec![section(0x30, 0)];
        let ranges = read_debug_frame(&bytes, &sections, true);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0x1_0000_1100);
        assert_eq!(ranges[0].size, 0x40);
    }

    #[test]
    fn version_one_32bit_defaults_to_four_byte_addresses() {
        let mut bytes = vec![0u8; 0x30];
        // CIE v1：id4 + ver1 + aug1 + code1 + data1 + ret1 = 9 字节体。
        put32(&mut bytes, 0, 9);
        put32(&mut bytes, 4, 0xffff_ffff);
        bytes[8] = 1;
        bytes[9] = 0;
        bytes[10] = 1;
        bytes[11] = 0x7c; // data align -4
        bytes[12] = 8;
        // FDE：id4 + pc4 + range4 = 12 字节体。
        put32(&mut bytes, 13, 12);
        put32(&mut bytes, 17, 0);
        put32(&mut bytes, 21, 0x1010);
        put32(&mut bytes, 25, 0x20);
        let sections = vec![section(0x30, 0)];
        let ranges = read_debug_frame(&bytes, &sections, false);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0x1010);
        assert_eq!(ranges[0].size, 0x20);
    }

    #[test]
    fn augmentation_zr_pcrel_pointer() {
        let mut bytes = vec![0u8; 0x40];
        // CIE v4，augmentation "zR"：body = id4+ver1+aug3+code1+data1+ret1+addr1+seg1+auglen1+enc1 = 15。
        put32(&mut bytes, 0, 15);
        put32(&mut bytes, 4, 0xffff_ffff);
        bytes[8] = 4;
        bytes[9] = b'z';
        bytes[10] = b'R';
        bytes[11] = 0;
        bytes[12] = 8;
        bytes[13] = 0;
        bytes[14] = 1;
        bytes[15] = 0x78;
        bytes[16] = 30;
        bytes[17] = 1; // augmentation 长度
        bytes[18] = 0x1b; // pcrel | sdata4
        // FDE：id4 + pcrel sdata4 + range4 = 12；节地址 0x2000，字段在 offset 23。
        put32(&mut bytes, 19, 12);
        put32(&mut bytes, 23, 0);
        let field_addr = 0x2000u64 + 27;
        put32(&mut bytes, 27, 0x1000u32.wrapping_sub(field_addr as u32));
        put32(&mut bytes, 31, 0x40);
        let sections = vec![section(0x40, 0x2000)];
        let ranges = read_debug_frame(&bytes, &sections, true);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0x1000);
        assert_eq!(ranges[0].size, 0x40);
    }

    #[test]
    fn unknown_augmentation_is_skipped_not_guessed() {
        let mut bytes = vec![0u8; 0x20];
        put32(&mut bytes, 0, 11);
        put32(&mut bytes, 4, 0xffff_ffff);
        bytes[8] = 4;
        bytes[9] = b'z';
        bytes[10] = b'X';
        let sections = vec![section(0x20, 0)];
        assert!(read_debug_frame(&bytes, &sections, true).is_empty());
    }
}
