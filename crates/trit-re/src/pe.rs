//! PE32/PE32+ 的最小解析：只读逆向需要的结构，不做反汇编。
//!
//! 边界原则：任何一个字段读不到就返回 `None`/`ParseError`，**不猜**。
//! 导入表只要有一个描述符读坏，整体返回 `None`——半个导入表会漏掉能力证据，
//! 那比"未知"更危险。

use std::fmt;

use crate::entropy;

mod overlay;

use overlay::parse_overlay;

/// PE 解析失败原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError(pub &'static str);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ParseError {}

const MAX_SECTIONS: usize = 96;
const MAX_DLLS: usize = 512;
const MAX_THUNKS: usize = 4096;
const MAX_EXPORTS: usize = 8192;
const CERT_OID_SIGNED_DATA: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02,
];

/// 一个节。
#[derive(Debug, Clone)]
pub struct Section {
    /// 节名（8 字节，遇 NUL 截断）。
    pub name: String,
    /// 虚拟大小。
    pub virtual_size: u32,
    /// 虚拟地址（RVA）。
    pub virtual_address: u32,
    /// 文件中的原始大小。
    pub raw_size: u32,
    /// 文件中的原始偏移。
    pub raw_ptr: u32,
    /// 节属性位。
    pub characteristics: u32,
    /// 原始数据的香农熵；数据越界时为 `None`。
    pub entropy: Option<f64>,
}

impl Section {
    /// 是否可执行。
    pub fn executable(&self) -> bool {
        self.characteristics & 0x2000_0000 != 0
    }

    /// 是否可写。
    pub fn writable(&self) -> bool {
        self.characteristics & 0x8000_0000 != 0
    }

    /// 映射大小（取虚拟/原始较大者）。
    pub fn mapped_size(&self) -> u32 {
        self.virtual_size.max(self.raw_size)
    }
}

/// 一个导入函数；槽位是 IAT 里那一格（调用点按它匹配）。
#[derive(Debug, Clone)]
pub struct ImportThunk {
    /// 函数名；按序号导入的记为 `#<序号>`。
    pub name: String,
    /// IAT 槽位 RVA（由 FirstThunk 起算）；FirstThunk 缺失时为 `None`。
    pub slot_rva: Option<u32>,
}

/// 一个导入 DLL 及其函数。
#[derive(Debug, Clone)]
pub struct ImportDll {
    /// DLL 名（按导入表原样）。
    pub name: String,
    /// 函数表。
    pub functions: Vec<ImportThunk>,
}

/// `.pdata` 里的一条运行时函数记录（x86-64 的精确函数边界）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeFunction {
    /// 起始 RVA。
    pub begin: u32,
    /// 结束 RVA（不含）。
    pub end: u32,
}

/// 叠加段（最后一个节之后的数据）。
#[derive(Debug, Clone)]
pub struct Overlay {
    /// 起始偏移。
    pub offset: usize,
    /// 字节数。
    pub size: usize,
    /// 熵。
    pub entropy: f64,
}

/// PE 结构摘要。
#[derive(Debug, Clone)]
pub struct PeInfo {
    /// 是否 PE32+。
    pub plus: bool,
    /// COFF machine。
    pub machine: u16,
    /// COFF 时间戳。
    pub timestamp: u32,
    /// 子系统。
    pub subsystem: u16,
    /// 入口 RVA。
    pub entry_rva: u32,
    /// 映像基址。
    pub image_base: u64,
    /// 节表。
    pub sections: Vec<Section>,
    /// 导入表；`None` 表示解析不完整/不可信。
    pub imports: Option<Vec<ImportDll>>,
    /// 导出名（尽力而为）。
    pub exports: Vec<String>,
    /// 调试目录里的 PDB 路径。
    pub debug_pdb: Option<String>,
    /// 是否存在安全目录（Authenticode）。
    pub has_security_dir: bool,
    /// 安全目录里能否找到 signedData 结构。
    pub has_signed_data: bool,
    /// DOS stub 里是否有 Rich 头。
    pub rich_header: bool,
    /// `.pdata` 函数表；`None` 表示没有异常目录或无法解析。
    pub runtime_functions: Option<Vec<RuntimeFunction>>,
    /// 是否存在 CLR（.NET）目录。
    pub clr_dir_present: bool,
    /// CLR 头；`None` 表示目录存在但无法解析。
    pub clr: Option<ClrHeader>,
    /// 是否存在 TLS 目录。
    pub tls_dir_present: bool,
    /// TLS 回调 RVA 列表；`None` 表示目录存在但回调数组无法解析。
    pub tls_callbacks: Option<Vec<u32>>,
    /// 叠加段。
    pub overlay: Option<Overlay>,
    /// 头部大小（SizeOfHeaders）。
    pub headers_size: u32,
}

/// CLR（.NET）头摘录。
#[derive(Debug, Clone)]
pub struct ClrHeader {
    /// 元数据里的运行时版本串（如 `v4.0.30319`）。
    pub runtime_version: Option<String>,
    /// `IMAGE_COR20_HEADER.Flags`。
    pub flags: u32,
    /// 入口 token（托管入口）或 RVA。
    pub entry_token: u32,
}

/// 解析 PE；不是 PE 或结构损坏时返回错误。
pub fn parse(bytes: &[u8]) -> Result<PeInfo, ParseError> {
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        return Err(ParseError("不是 MZ 头"));
    }
    let pe_off = u32_at(bytes, 0x3c).ok_or(ParseError("读不到 e_lfanew"))? as usize;
    if bytes.get(pe_off..pe_off + 4) != Some(b"PE\0\0") {
        return Err(ParseError("e_lfanew 处不是 PE 签名"));
    }
    let coff = pe_off + 4;
    let machine = u16_at(bytes, coff).ok_or(ParseError("COFF 头截断"))?;
    let num_sections = u16_at(bytes, coff + 2).ok_or(ParseError("COFF 头截断"))? as usize;
    let timestamp = u32_at(bytes, coff + 4).ok_or(ParseError("COFF 头截断"))?;
    let opt_size = u16_at(bytes, coff + 16).ok_or(ParseError("COFF 头截断"))? as usize;
    let opt = coff + 20;
    let magic = u16_at(bytes, opt).ok_or(ParseError("可选头截断"))?;
    let plus = match magic {
        0x10b => false,
        0x20b => true,
        _ => return Err(ParseError("未知的可选头 magic")),
    };
    let entry_rva = u32_at(bytes, opt + 16).ok_or(ParseError("可选头截断"))?;
    let image_base = if plus {
        u64_at(bytes, opt + 24).ok_or(ParseError("可选头截断"))?
    } else {
        u32_at(bytes, opt + 28).ok_or(ParseError("可选头截断"))? as u64
    };
    let headers_size = u32_at(bytes, opt + 60).ok_or(ParseError("可选头截断"))?;
    let subsystem = u16_at(bytes, opt + 68).ok_or(ParseError("可选头截断"))?;
    let dir_count = u32_at(bytes, if plus { opt + 108 } else { opt + 92 })
        .ok_or(ParseError("可选头截断"))? as usize;
    let dirs_off = if plus { opt + 112 } else { opt + 96 };
    let dir_count = dir_count.min(16);

    let mut dirs = [(0u32, 0u32); 16];
    for (index, slot) in dirs.iter_mut().enumerate().take(dir_count) {
        let base = dirs_off + index * 8;
        *slot = (
            u32_at(bytes, base).ok_or(ParseError("数据目录截断"))?,
            u32_at(bytes, base + 4).ok_or(ParseError("数据目录截断"))?,
        );
    }

    let sections_off = opt + opt_size;
    let section_count = num_sections.min(MAX_SECTIONS);
    let mut sections = Vec::with_capacity(section_count);
    for index in 0..section_count {
        let base = sections_off + index * 40;
        let name_raw = bytes.get(base..base + 8).ok_or(ParseError("节表截断"))?;
        let name = String::from_utf8_lossy(
            &name_raw[..name_raw.iter().position(|b| *b == 0).unwrap_or(8)],
        )
        .into_owned();
        let virtual_size = u32_at(bytes, base + 8).ok_or(ParseError("节表截断"))?;
        let virtual_address = u32_at(bytes, base + 12).ok_or(ParseError("节表截断"))?;
        let raw_size = u32_at(bytes, base + 16).ok_or(ParseError("节表截断"))?;
        let raw_ptr = u32_at(bytes, base + 20).ok_or(ParseError("节表截断"))?;
        let characteristics = u32_at(bytes, base + 36).ok_or(ParseError("节表截断"))?;
        let entropy = raw_slice(bytes, raw_ptr, raw_size).map(entropy::shannon);
        sections.push(Section {
            name,
            virtual_size,
            virtual_address,
            raw_size,
            raw_ptr,
            characteristics,
            entropy,
        });
    }

    let imports = parse_imports(bytes, dirs[1], &sections, headers_size, plus);
    let exports = parse_exports(bytes, dirs[0], &sections, headers_size);
    let debug_pdb = parse_debug_pdb(bytes, dirs[6], &sections, headers_size);
    let runtime_functions =
        parse_runtime_functions(bytes, dirs[3], &sections, headers_size, machine);
    let (clr_dir_present, clr) = parse_clr(bytes, dirs[14], &sections, headers_size);
    let (tls_dir_present, tls_callbacks) =
        parse_tls(bytes, dirs[9], &sections, headers_size, image_base, plus);
    let (has_security_dir, has_signed_data) = parse_security(bytes, dirs[4]);
    let rich_header = bytes
        .get(0x40..pe_off)
        .map(|stub| stub.windows(4).any(|w| w == b"Rich"))
        .unwrap_or(false);
    let overlay = parse_overlay(bytes, &sections, headers_size, dirs[4]);

    Ok(PeInfo {
        plus,
        machine,
        timestamp,
        subsystem,
        entry_rva,
        image_base,
        sections,
        imports,
        exports,
        debug_pdb,
        runtime_functions,
        clr_dir_present,
        clr,
        tls_dir_present,
        tls_callbacks,
        has_security_dir,
        has_signed_data,
        rich_header,
        overlay,
        headers_size,
    })
}

impl PeInfo {
    /// 把 RVA 映射到文件偏移。
    pub fn rva_to_offset(&self, rva: u32) -> Option<usize> {
        rva_to_offset(rva, &self.sections, self.headers_size)
    }

    /// 入口点是否落在某个可执行节内；映射不到时返回 `None`。
    pub fn entry_in_executable(&self) -> Option<bool> {
        if self.entry_rva == 0 {
            return None;
        }
        self.rva_to_offset(self.entry_rva)?;
        Some(
            self.sections
                .iter()
                .any(|s| s.executable() && rva_in_section(self.entry_rva, s)),
        )
    }

    /// 架构短名。
    pub fn arch(&self) -> &'static str {
        machine_name(self.machine)
    }

    /// 格式短名。
    pub fn format(&self) -> &'static str {
        if self.plus { "pe32+" } else { "pe32" }
    }
}

fn rva_in_section(rva: u32, section: &Section) -> bool {
    rva >= section.virtual_address
        && rva
            < section
                .virtual_address
                .saturating_add(section.mapped_size())
}

fn rva_to_offset(rva: u32, sections: &[Section], headers_size: u32) -> Option<usize> {
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

fn raw_slice(bytes: &[u8], raw_ptr: u32, raw_size: u32) -> Option<&[u8]> {
    if raw_size == 0 || raw_ptr == 0 {
        return None;
    }
    let start = raw_ptr as usize;
    let end = start.checked_add(raw_size as usize)?;
    bytes.get(start..end)
}

fn parse_imports(
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

fn parse_thunks(
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

fn parse_exports(
    bytes: &[u8],
    dir: (u32, u32),
    sections: &[Section],
    headers_size: u32,
) -> Vec<String> {
    if dir.0 == 0 {
        return Vec::new();
    }
    let Some(base) = rva_to_offset(dir.0, sections, headers_size) else {
        return Vec::new();
    };
    let Some(name_count) = u32_at(bytes, base + 24) else {
        return Vec::new();
    };
    let Some(names_rva) = u32_at(bytes, base + 32) else {
        return Vec::new();
    };
    let Some(names_off) = rva_to_offset(names_rva, sections, headers_size) else {
        return Vec::new();
    };
    let count = (name_count as usize).min(MAX_EXPORTS);
    let mut exports = Vec::with_capacity(count);
    for index in 0..count {
        let Some(name_rva) = u32_at(bytes, names_off + index * 4) else {
            break;
        };
        let Some(name_off) = rva_to_offset(name_rva, sections, headers_size) else {
            continue;
        };
        if let Some(name) = cstr(bytes, name_off, 512) {
            exports.push(name);
        }
    }
    exports
}

fn parse_debug_pdb(
    bytes: &[u8],
    dir: (u32, u32),
    sections: &[Section],
    headers_size: u32,
) -> Option<String> {
    if dir.0 == 0 {
        return None;
    }
    let base = rva_to_offset(dir.0, sections, headers_size)?;
    for index in 0..64 {
        let off = base + index * 28;
        let debug_type = u32_at(bytes, off + 12)?;
        let size = u32_at(bytes, off + 16)?;
        let pointer = u32_at(bytes, off + 24)?;
        if debug_type == 0 && size == 0 && pointer == 0 {
            break;
        }
        if debug_type != 2 {
            continue;
        }
        let data_off = pointer as usize;
        let magic = bytes.get(data_off..data_off + 4);
        match magic {
            Some(b"RSDS") => return cstr(bytes, data_off + 24, 512),
            Some(b"NB10") => return cstr(bytes, data_off + 16, 512),
            _ => continue,
        }
    }
    None
}

fn parse_clr(
    bytes: &[u8],
    dir: (u32, u32),
    sections: &[Section],
    headers_size: u32,
) -> (bool, Option<ClrHeader>) {
    if dir.0 == 0 {
        return (false, None);
    }
    let Some(base) = rva_to_offset(dir.0, sections, headers_size) else {
        return (true, None);
    };
    let Some(cb) = u32_at(bytes, base) else {
        return (true, None);
    };
    if cb < 72 {
        return (true, None);
    }
    let metadata_rva = u32_at(bytes, base + 8).unwrap_or(0);
    let flags = u32_at(bytes, base + 16).unwrap_or(0);
    let entry_token = u32_at(bytes, base + 20).unwrap_or(0);
    let runtime_version = metadata_version(bytes, metadata_rva, sections, headers_size);
    (
        true,
        Some(ClrHeader {
            runtime_version,
            flags,
            entry_token,
        }),
    )
}

fn metadata_version(
    bytes: &[u8],
    rva: u32,
    sections: &[Section],
    headers_size: u32,
) -> Option<String> {
    if rva == 0 {
        return None;
    }
    let base = rva_to_offset(rva, sections, headers_size)?;
    if u32_at(bytes, base)? != 0x424a_5342 {
        return None;
    }
    let length = u32_at(bytes, base + 12)? as usize;
    if length == 0 || length > 4096 {
        return None;
    }
    let raw = bytes.get(base + 16..base + 16 + length)?;
    let text = String::from_utf8_lossy(raw)
        .trim_end_matches(['\0', ' '])
        .to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn parse_tls(
    bytes: &[u8],
    dir: (u32, u32),
    sections: &[Section],
    headers_size: u32,
    image_base: u64,
    plus: bool,
) -> (bool, Option<Vec<u32>>) {
    if dir.0 == 0 {
        return (false, None);
    }
    let Some(base) = rva_to_offset(dir.0, sections, headers_size) else {
        return (true, None);
    };
    let callbacks = if plus {
        u64_at(bytes, base + 24)
    } else {
        u32_at(bytes, base + 12).map(|value| value as u64)
    };
    let Some(callbacks) = callbacks else {
        return (true, None);
    };
    if callbacks == 0 {
        return (true, Some(Vec::new()));
    }
    let Some(callbacks_rva) = callbacks
        .checked_sub(image_base)
        .and_then(|value| u32::try_from(value).ok())
    else {
        return (true, None);
    };
    let Some(mut offset) = rva_to_offset(callbacks_rva, sections, headers_size) else {
        return (true, None);
    };
    let width = if plus { 8 } else { 4 };
    let mut out = Vec::new();
    for _ in 0..64 {
        let value = if plus {
            u64_at(bytes, offset)
        } else {
            u32_at(bytes, offset).map(|value| value as u64)
        };
        let Some(value) = value else {
            break;
        };
        if value == 0 {
            return (true, Some(out));
        }
        if let Some(callback) = value
            .checked_sub(image_base)
            .and_then(|value| u32::try_from(value).ok())
            && rva_to_offset(callback, sections, headers_size).is_some()
        {
            out.push(callback);
        }
        offset += width;
    }
    (true, Some(out))
}

fn parse_runtime_functions(
    bytes: &[u8],
    dir: (u32, u32),
    sections: &[Section],
    headers_size: u32,
    machine: u16,
) -> Option<Vec<RuntimeFunction>> {
    if dir.0 == 0 {
        return None;
    }
    let base = rva_to_offset(dir.0, sections, headers_size)?;
    // x86-64 的条目 12 字节（begin/end/unwind）；ARM64/ARM 是 8 字节（begin/unwind），
    // 结束地址要靠下一条的 begin 推。
    let entry_size = match machine {
        0x8664 => 12,
        0xaa64 | 0x01c0 | 0x01c4 => 8,
        _ => return None,
    };
    // 上限只防坏目录：真实大二进制（Node/Deno）的 `.pdata` 远超 6.5 万条。
    let count = (dir.1 as usize / entry_size).min(1_048_576);
    if count == 0 {
        return None;
    }
    let mut begins = Vec::with_capacity(count);
    for index in 0..count {
        let off = base + index * entry_size;
        let begin = u32_at(bytes, off)?;
        if begin == 0 {
            break;
        }
        begins.push(begin);
    }
    if begins.is_empty() {
        return None;
    }
    if entry_size == 12 {
        let mut functions = Vec::with_capacity(begins.len());
        for (index, begin) in begins.iter().enumerate() {
            let end = u32_at(bytes, base + index * 12 + 4)?;
            if end <= *begin {
                return None;
            }
            functions.push(RuntimeFunction { begin: *begin, end });
        }
        return Some(functions);
    }
    // ARM 系：end = 下一函数 begin；最后一个用可执行节末尾兜底。
    begins.sort_unstable();
    begins.dedup();
    let image_end = sections
        .iter()
        .filter(|section| section.executable())
        .map(|section| {
            section
                .virtual_address
                .saturating_add(section.mapped_size())
        })
        .max()?;
    let mut functions = Vec::with_capacity(begins.len());
    for (index, begin) in begins.iter().enumerate() {
        let end = begins.get(index + 1).copied().unwrap_or(image_end);
        if end <= *begin {
            return None;
        }
        functions.push(RuntimeFunction { begin: *begin, end });
    }
    Some(functions)
}

fn parse_security(bytes: &[u8], dir: (u32, u32)) -> (bool, bool) {
    if dir.0 == 0 || dir.1 <= 8 {
        return (false, false);
    }
    let start = dir.0 as usize;
    let Some(blob) = bytes.get(start..start.saturating_add(dir.1 as usize)) else {
        return (true, false);
    };
    let has_signed_data = blob
        .windows(CERT_OID_SIGNED_DATA.len())
        .any(|window| window == CERT_OID_SIGNED_DATA);
    (true, has_signed_data)
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

/// COFF machine → 架构名。
pub fn machine_name(machine: u16) -> &'static str {
    match machine {
        0x014c => "x86",
        0x8664 => "x86-64",
        0x01c0 | 0x01c4 => "arm",
        0xaa64 => "arm64",
        0x0200 => "ia64",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{ParseError, parse};

    #[test]
    fn rejects_non_pe() {
        assert_eq!(parse(b"MZ").unwrap_err(), ParseError("不是 MZ 头"));
        assert_eq!(parse(b"").unwrap_err(), ParseError("不是 MZ 头"));
    }

    #[test]
    fn rejects_bad_lfanew() {
        let mut bytes = vec![0u8; 0x80];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x7fffu32.to_le_bytes());
        assert_eq!(
            parse(&bytes).unwrap_err(),
            ParseError("e_lfanew 处不是 PE 签名")
        );
    }

    #[test]
    fn truncated_optional_header_is_an_error_not_a_panic() {
        let mut bytes = vec![0u8; 0x100];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
        assert!(parse(&bytes).is_err());
    }
}
