//! 标准 PE 导入表（从 `pe.rs` 拆出，守单文件行数预算）。

use super::{
    ImportDll, ImportThunk, MAX_DLLS, MAX_THUNKS, Section, cstr, rva_to_offset, u32_at, u64_at,
};

pub(super) fn parse_imports(
    bytes: &[u8],
    dir: (u32, u32),
    sections: &[Section],
    headers_size: u32,
    plus: bool,
) -> Option<Vec<ImportDll>> {
    if dir.0 == 0 {
        return Some(Vec::new());
    }
    let base = rva_to_offset(dir.0, sections, headers_size)?;
    let mut dlls = Vec::new();
    for index in 0..MAX_DLLS {
        let off = base + index * 20;
        let original_first_thunk = u32_at(bytes, off)?;
        let time_stamp = u32_at(bytes, off + 4)?;
        let forwarder = u32_at(bytes, off + 8)?;
        let name_rva = u32_at(bytes, off + 12)?;
        let first_thunk = u32_at(bytes, off + 16)?;
        if original_first_thunk == 0
            && time_stamp == 0
            && forwarder == 0
            && name_rva == 0
            && first_thunk == 0
        {
            return Some(dlls);
        }
        let name_off = rva_to_offset(name_rva, sections, headers_size)?;
        let name = cstr(bytes, name_off, 256)?;
        let name_thunk_rva = if original_first_thunk != 0 {
            original_first_thunk
        } else {
            first_thunk
        };
        let functions = parse_thunks(
            bytes,
            name_thunk_rva,
            first_thunk,
            sections,
            headers_size,
            plus,
        )?;
        dlls.push(ImportDll { name, functions });
    }
    // 描述符表没遇到全零结束标记：视为不可信。
    None
}

pub(super) fn parse_thunks(
    bytes: &[u8],
    name_thunk_rva: u32,
    first_thunk_rva: u32,
    sections: &[Section],
    headers_size: u32,
    plus: bool,
) -> Option<Vec<ImportThunk>> {
    if name_thunk_rva == 0 {
        return Some(Vec::new());
    }
    let base = rva_to_offset(name_thunk_rva, sections, headers_size)?;
    let width = if plus { 8 } else { 4 };
    let mut functions = Vec::new();
    for index in 0..MAX_THUNKS {
        let off = base + index * width;
        let raw = if plus {
            u64_at(bytes, off)?
        } else {
            u32_at(bytes, off)? as u64
        };
        if raw == 0 {
            return Some(functions);
        }
        let ordinal_bit = if plus { 1u64 << 63 } else { 1u64 << 31 };
        let slot_rva = if first_thunk_rva == 0 {
            None
        } else {
            Some(first_thunk_rva.checked_add(index as u32 * width as u32)?)
        };
        if raw & ordinal_bit != 0 {
            functions.push(ImportThunk {
                name: format!("#{}", raw & 0xffff),
                slot_rva,
            });
            continue;
        }
        let name_off = rva_to_offset(raw as u32, sections, headers_size)?;
        // hint(2) + 名字
        functions.push(ImportThunk {
            name: cstr(bytes, name_off.checked_add(2)?, 256)?,
            slot_rva,
        });
    }
    None
}
