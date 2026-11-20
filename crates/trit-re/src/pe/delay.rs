//! PE delay-load 导入表（从 `pe.rs` 拆出，守单文件行数预算）。

use super::imports::parse_thunks;
use super::{ImportDll, MAX_DLLS, Section, cstr, rva_to_offset, u32_at};

/// 延迟加载导入（IMAGE_DELAYLOAD_DESCRIPTOR，32 字节/项）。
///
/// 安全上不能省略这张表：把 `VirtualAllocEx` / `WriteProcessMemory` /
/// `CreateRemoteThread` 放进 delay-load 描述符，和放进标准导入表的执行能力
/// 完全一样；只看 `dirs[1]` 会得到 `injection=false` 的假阴性。
///
/// `Attributes.dlattrRva`（bit 0）决定描述符字段是 RVA 还是 VA；VA 形态
/// 按 `image_base` 折回 RVA。未知属性位 / 任一字段读坏 ⇒ 整表 `None`，
/// 与标准导入表同样的 fail-closed 口径。
pub(super) fn parse_delay_imports(
    bytes: &[u8],
    dir: (u32, u32),
    sections: &[Section],
    headers_size: u32,
    plus: bool,
    image_base: u64,
) -> Option<Vec<ImportDll>> {
    if dir.0 == 0 {
        return Some(Vec::new());
    }
    let base = rva_to_offset(dir.0, sections, headers_size)?;
    let mut dlls = Vec::new();
    for index in 0..MAX_DLLS {
        let off = base.checked_add(index.checked_mul(32)?)?;
        let attributes = u32_at(bytes, off)?;
        let dll_name = u32_at(bytes, off + 4)?;
        let module_handle = u32_at(bytes, off + 8)?;
        let import_address_table = u32_at(bytes, off + 12)?;
        let import_name_table = u32_at(bytes, off + 16)?;
        let bound_import_address_table = u32_at(bytes, off + 20)?;
        let unload_information_table = u32_at(bytes, off + 24)?;
        let time_stamp = u32_at(bytes, off + 28)?;
        if attributes == 0
            && dll_name == 0
            && module_handle == 0
            && import_address_table == 0
            && import_name_table == 0
            && bound_import_address_table == 0
            && unload_information_table == 0
            && time_stamp == 0
        {
            return Some(dlls);
        }
        // 只认官方定义的 bit 0；其余位语义未知，按不可信拒绝。
        if attributes & !1 != 0 {
            return None;
        }
        if dll_name == 0 {
            return None;
        }
        let rva_based = attributes & 1 != 0;
        let to_rva = |raw: u32| delay_field_to_rva(raw, rva_based, image_base);
        let name_rva = to_rva(dll_name)?;
        let name_off = rva_to_offset(name_rva, sections, headers_size)?;
        let name = cstr(bytes, name_off, 256)?;
        let functions = parse_thunks(
            bytes,
            to_rva(import_name_table)?,
            to_rva(import_address_table)?,
            sections,
            headers_size,
            plus,
        )?;
        dlls.push(ImportDll { name, functions });
    }
    // 描述符表没遇到全零结束标记：视为不可信。
    None
}

fn delay_field_to_rva(raw: u32, rva_based: bool, image_base: u64) -> Option<u32> {
    if raw == 0 {
        return Some(0);
    }
    if rva_based {
        return Some(raw);
    }
    let rva = u64::from(raw).checked_sub(image_base)?;
    u32::try_from(rva).ok()
}

/// 合并标准导入与 delay 导入；任一表声明存在却解析不完整 ⇒ 整体不可信。
pub(super) fn merge(
    normal: Option<Vec<ImportDll>>,
    delay: Option<Vec<ImportDll>>,
) -> Option<Vec<ImportDll>> {
    match (normal, delay) {
        (Some(mut normal), Some(delay)) => {
            normal.extend(delay);
            Some(normal)
        }
        _ => None,
    }
}
