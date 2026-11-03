//! COFF 符号表 → 函数符号。
//!
//! 独立成文件是为了守住 800 行预算：`pe.rs` 加上符号表表面后到 878 行。
//! 公开路径不变——`pe::CoffFunction` 仍由 `pe.rs` 的 `pub use` 提供。

use super::{Section, u16_at, u32_at};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoffFunction {
    /// 符号 RVA（映像文件里 Value 按 RVA 解释；自校验落在可执行节内）。
    pub rva: u32,
    /// 符号名（短名内联或串表偏移；串表缺失/越界时为 `None`）。
    pub name: Option<String>,
}

/// COFF 符号表 → 函数符号（PE 规范 §5.4）。
///
/// - 每条记录 18 字节：名字 8 字节（前 4 字节全 0 ⇒ 后 4 字节是串表偏移）、
///   Value(u32)、SectionNumber(i16)、Type(u16)、StorageClass(u8)、AuxCount(u8)；
/// - 函数 = SectionNumber > 0（指向可执行节）且 Type 高字节 = 0x20
///   （`IMAGE_SYM_DTYPE_FUNCTION`）；Aux 记录整条跳过；
/// - **映像文件里 Value 按 RVA 解释**（规范 §5.4.2），自校验落在所指节内——
///   对不上（比如表被剥过或值语义不对）的符号直接丢弃；一条都解析不出且
///   表结构可疑时返回 `None`（整表不可信，边界退回启发式）；
/// - 串表紧跟符号表（首 4 字节 = 总大小），名字越界记 `None` 不废整表。
pub(super) fn parse_coff_functions(
    bytes: &[u8],
    sym_ptr: u32,
    num_symbols: u32,
    sections: &[Section],
) -> Option<Vec<CoffFunction>> {
    if sym_ptr == 0 || num_symbols == 0 {
        return None;
    }
    let table = sym_ptr as usize;
    let count = num_symbols as usize;
    let table_end = table.checked_add(count.checked_mul(18)?)?;
    if table_end > bytes.len() {
        return None;
    }
    let mut functions = Vec::new();
    let mut seen = 0usize;
    let mut index = 0usize;
    while index < count {
        let base = table + index * 18;
        let name_raw = bytes.get(base..base + 8)?;
        let value = u32_at(bytes, base + 8)?;
        // 18 字节布局：Name(8) Value(4) SectionNumber(+12) Type(+14)
        // StorageClass(+16) NumAux(+17)。
        let section_number = i16::from_le_bytes([bytes[base + 12], bytes[base + 13]]);
        let type_field = u16_at(bytes, base + 14)?;
        let aux = *bytes.get(base + 17)? as usize;
        index += 1 + aux;
        seen += 1;
        let is_function = section_number > 0 && (type_field >> 8) == 0x02; // IMAGE_SYM_DTYPE_FUNCTION（字段值 = 2 << 8 = 0x0200）
        let Some(section) = (is_function)
            .then(|| sections.get(section_number as usize - 1))
            .flatten()
            .filter(|section| section.executable())
        else {
            continue;
        };
        // 值按 RVA 解释；自校验：必须落在所指可执行节内（对 strip 过/非常规
        // 的表宁可丢符号也不给假边界）。
        let rva = value;
        let section_lo = section.virtual_address;
        let section_hi = section_lo.checked_add(section.virtual_size.max(section.raw_size))?;
        if rva < section_lo || rva >= section_hi {
            continue;
        }
        let name = read_coff_name(bytes, table_end, name_raw);
        functions.push(CoffFunction { rva, name });
    }
    if seen == 0 {
        return None;
    }
    functions.sort_by_key(|function| function.rva);
    functions.dedup_by(|left, right| left.rva == right.rva);
    Some(functions)
}

/// 读 COFF 符号名：8 字节内联短名，或（前 4 字节全 0 时）串表偏移。
/// 越界/非 UTF-8 都记 `None`——名字只用于展示，不影响边界判定。
fn read_coff_name(bytes: &[u8], string_table: usize, raw: &[u8]) -> Option<String> {
    if raw[..4] == [0, 0, 0, 0] {
        let offset = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize;
        let start = string_table.checked_add(offset)?;
        let rest = bytes.get(start..)?;
        let end = rest.iter().position(|byte| *byte == 0)?;
        std::str::from_utf8(&rest[..end])
            .ok()
            .map(|name| name.to_string())
            .filter(|name| !name.is_empty())
    } else {
        let len = raw.iter().position(|byte| *byte == 0).unwrap_or(8);
        std::str::from_utf8(&raw[..len])
            .ok()
            .map(|name| name.to_string())
            .filter(|name| !name.is_empty())
    }
}
