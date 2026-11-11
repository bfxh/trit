//! RVA ↔ 文件偏移映射（从 pe.rs 拆出——模块预算棘轮纪律：拆文件不抬台账）。

use super::Section;

pub(super) fn rva_to_offset(rva: u32, sections: &[Section], headers_size: u32) -> Option<usize> {
    if rva < headers_size {
        return Some(rva as usize);
    }
    for section in sections {
        if rva_in_section(rva, section) && section.raw_ptr != 0 {
            let delta = rva - section.virtual_address;
            if delta < section.raw_size {
                return Some(section.raw_ptr as usize + delta as usize);
            }
        }
    }
    None
}

/// RVA 是否落在节的虚拟区间内。
pub(super) fn rva_in_section(rva: u32, section: &Section) -> bool {
    rva >= section.virtual_address
        && rva
            < section
                .virtual_address
                .saturating_add(section.mapped_size())
}
