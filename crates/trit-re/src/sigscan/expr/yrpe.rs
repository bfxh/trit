//! 官方口径的 PE 视图（照 libyara 4.5.4 `pe.c` / `pe_utils.c` 逐句镜像）：
//! 头部标量、节表、数据目录、overlay，以及 `pe_rva_to_offset`。与
//! `crate::pe` 的取证解析器**刻意分开**——那边的口径是"尽量读懂"，这边
//! 的口径是"官方 YARA 看到什么"（上限、对齐怪癖、越界处理都照搬），
//! 否则对拍必然出 DIFF。只读字节，不执行样本。

use super::node::Node;
use super::{yrdbg, yrexp, yrimp, yrres};

const MAX_PE_SECTIONS: usize = 96;
const SECTION_SIZE: usize = 40;

pub(super) struct Pe<'a> {
    pub(super) d: &'a [u8],
    /// NT 头（`PE\0\0` 签名）的文件偏移。
    nt: usize,
    pub(super) is64: bool,
}

impl<'a> Pe<'a> {
    /// `pe_get_header`：不满足即"不是 PE"（官方此时只有 `is_pe == 0`）。
    pub(super) fn parse(d: &'a [u8]) -> Option<Self> {
        if d.len() < 64 || d[..2] != *b"MZ" {
            return None;
        }
        let lfanew = i32::from_le_bytes(d[0x3c..0x40].try_into().ok()?);
        let nt = usize::try_from(lfanew).ok()?;
        let mut headers = nt.checked_add(4 + 20)?;
        if d.len() < headers || d[nt..nt + 4] != *b"PE\0\0" {
            return None;
        }
        if d.len() < headers.checked_add(224)? {
            return None;
        }
        let is64 = d[nt + 24..nt + 26] == [0x0b, 0x02];
        headers += if is64 { 240 } else { 224 };
        (d.len() >= headers).then_some(Self { d, nt, is64 })
    }

    pub(super) fn len(&self) -> u64 {
        self.d.len() as u64
    }

    /// `fits_in_pe(pe, data + at, size)`
    pub(super) fn fits(&self, at: u64, size: u64) -> bool {
        size <= self.len() && at <= self.len() - size
    }

    pub(super) fn u16(&self, at: u64) -> Option<u16> {
        let at = usize::try_from(at).ok()?;
        let bytes = self.d.get(at..at.checked_add(2)?)?;
        Some(u16::from_le_bytes(bytes.try_into().ok()?))
    }

    pub(super) fn u32(&self, at: u64) -> Option<u32> {
        let at = usize::try_from(at).ok()?;
        let bytes = self.d.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    pub(super) fn u64(&self, at: u64) -> Option<u64> {
        let at = usize::try_from(at).ok()?;
        let bytes = self.d.get(at..at.checked_add(8)?)?;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    /// `at` 起到 NUL / 文件尾 / `max` 字节为止的 C 串。
    pub(super) fn cstr(&self, at: u64, max: usize) -> &'a [u8] {
        let rest = usize::try_from(at)
            .ok()
            .and_then(|at| self.d.get(at..))
            .unwrap_or(&[]);
        let rest = &rest[..rest.len().min(max)];
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        &rest[..end]
    }

    /// 已校验范围内的头部字段读取（`pe_get_header` 保证了可读）。
    fn head(&self, at: usize, size: usize) -> u64 {
        let mut raw = [0u8; 8];
        if let Some(bytes) = self.d.get(at..at + size) {
            raw[..size].copy_from_slice(bytes);
        }
        u64::from_le_bytes(raw)
    }

    fn file(&self, at: usize, size: usize) -> u64 {
        self.head(self.nt + 4 + at, size)
    }

    /// 可选头字段：`(PE32 偏移, PE32+ 偏移, PE32 宽度)`；PE32+ 下宽度 4 的
    /// 仍是 4，宽度 0 表示"随位数"（4 / 8）。
    fn opt(&self, at32: usize, at64: usize, size: usize) -> u64 {
        let (at, size) = match (self.is64, size) {
            (true, 0) => (at64, 8),
            (false, 0) => (at32, 4),
            (true, size) => (at64, size),
            (false, size) => (at32, size),
        };
        self.head(self.nt + 24 + at, size)
    }

    fn section_count(&self) -> usize {
        (self.file(2, 2) as usize).min(MAX_PE_SECTIONS)
    }

    fn section_at(&self, index: usize) -> u64 {
        (self.nt + 24 + self.file(16, 2) as usize + index * SECTION_SIZE) as u64
    }

    /// `pe_get_directory_entry`：**不**看 NumberOfRvaAndSizes，只要求目录项
    /// 本身落在文件内。返回 `(VirtualAddress, Size)`。
    pub(super) fn directory(&self, entry: usize) -> Option<(u32, u32)> {
        let base = self.nt + 24 + if self.is64 { 112 } else { 96 };
        let at = (base + entry * 8) as u64;
        self.fits(at, 8)
            .then(|| (self.u32(at).unwrap_or(0), self.u32(at + 4).unwrap_or(0)))
    }

    /// `pe_rva_to_offset`：失败 = -1。对齐怪癖照搬（FileAlignment 截到
    /// 0x200 取整、SectionAlignment ≥ 0x1000 时再按扇区取整、节表之前的
    /// RVA 直映射）。
    pub(super) fn rva_to_offset(&self, rva: u64) -> i64 {
        let mut lowest = 0xffff_ffffu64;
        let (mut sec_rva, mut sec_off, mut sec_raw) = (0u64, 0u32, 0u64);
        for index in 0..self.section_count() {
            let at = self.section_at(index);
            if !self.fits(at, SECTION_SIZE as u64) {
                return -1;
            }
            let word = |field: u64| u64::from(self.u32(at + field).unwrap_or(0));
            let (vsize, va, raw_size, raw_ptr) = (word(8), word(12), word(16), word(20));
            lowest = lowest.min(va);
            if rva >= va && rva - va < vsize.max(raw_size) && sec_rva <= va {
                let alignment = (self.opt(36, 36, 4) as u32).min(0x200);
                sec_rva = va;
                sec_off = raw_ptr as u32;
                sec_raw = raw_size;
                if alignment != 0 {
                    sec_off -= sec_off % alignment;
                }
                if self.opt(32, 32, 4) >= 0x1000 {
                    sec_off &= !0x1ff;
                }
            }
        }
        if rva < lowest {
            sec_rva = 0;
            sec_off = 0;
            sec_raw = self.len() & 0xffff_ffff;
        }
        if rva - sec_rva >= sec_raw {
            return -1;
        }
        let result = u64::from(sec_off).wrapping_add(rva - sec_rva);
        if result >= self.len() {
            -1
        } else {
            result as i64
        }
    }
}

/// 可选头标量表：`(字段名, PE32 偏移, PE32+ 偏移, 宽度)`，宽度 0 = 随位数。
const OPTIONAL: &[(&str, usize, usize, usize)] = &[
    ("entry_point_raw", 16, 16, 4),
    ("image_base", 28, 24, 0),
    ("number_of_rva_and_sizes", 92, 108, 4),
    ("opthdr_magic", 0, 0, 2),
    ("size_of_code", 4, 4, 4),
    ("size_of_initialized_data", 8, 8, 4),
    ("size_of_uninitialized_data", 12, 12, 4),
    ("base_of_code", 20, 20, 4),
    ("section_alignment", 32, 32, 4),
    ("file_alignment", 36, 36, 4),
    ("win32_version_value", 52, 52, 4),
    ("size_of_image", 56, 56, 4),
    ("size_of_headers", 60, 60, 4),
    ("checksum", 64, 64, 4),
    ("subsystem", 68, 68, 2),
    ("dll_characteristics", 70, 70, 2),
    ("size_of_stack_reserve", 72, 72, 0),
    ("size_of_stack_commit", 76, 80, 0),
    ("size_of_heap_reserve", 80, 88, 0),
    ("size_of_heap_commit", 84, 96, 0),
    ("loader_flags", 88, 104, 4),
];

const VERSIONS: &[(&str, usize, usize)] = &[
    ("linker_version", 2, 1),
    ("os_version", 40, 2),
    ("image_version", 44, 2),
    ("subsystem_version", 48, 2),
];

const FILE_HEADER: &[(&str, usize, usize)] = &[
    ("machine", 0, 2),
    ("number_of_sections", 2, 2),
    ("timestamp", 4, 4),
    ("pointer_to_symbol_table", 8, 4),
    ("number_of_symbols", 12, 4),
    ("size_of_optional_header", 16, 2),
    ("characteristics", 18, 2),
];

const SECTION_FIELDS: &[(&str, u64, bool)] = &[
    ("virtual_size", 8, false),
    ("virtual_address", 12, false),
    ("raw_data_size", 16, false),
    ("raw_data_offset", 20, false),
    ("pointer_to_relocations", 24, false),
    ("pointer_to_line_numbers", 28, false),
    ("number_of_relocations", 32, true),
    ("number_of_line_numbers", 34, true),
    ("characteristics", 36, false),
];

fn int(value: u64) -> Node {
    Node::Int(value as i64)
}

fn sections(pe: &Pe, fields: &mut Vec<(&'static str, Node)>) {
    let mut out = Vec::new();
    let (mut top_off, mut top_size) = (0u64, 0u64);
    for index in 0..pe.section_count() {
        let at = pe.section_at(index);
        if !pe.fits(at, SECTION_SIZE as u64) {
            break;
        }
        let raw = &pe.d[at as usize..at as usize + 8];
        let name = &raw[..raw.iter().rposition(|b| *b != 0).map_or(0, |last| last + 1)];
        // full_name：有 COFF 符号表且名字形如 "/123" 时要查串表——未建模
        let plain = pe.file(8, 4) == 0 || name.first() != Some(&b'/');
        let mut sec = vec![
            ("name", Node::Str(name.to_vec())),
            (
                "full_name",
                if plain {
                    Node::Str(name.to_vec())
                } else {
                    Node::Unknown
                },
            ),
        ];
        for (field, offset, short) in SECTION_FIELDS {
            let value = if *short {
                pe.u16(at + offset).map(u64::from)
            } else {
                pe.u32(at + offset).map(u64::from)
            };
            sec.push((*field, int(value.unwrap_or(0))));
        }
        let (raw_ptr, raw_size) = (
            u64::from(pe.u32(at + 20).unwrap_or(0)),
            u64::from(pe.u32(at + 16).unwrap_or(0)),
        );
        if raw_ptr + raw_size > top_off + top_size {
            top_off = raw_ptr;
            top_size = raw_size;
        }
        out.push(Node::Struct(sec));
    }
    fields.push(("sections", Node::Array(out)));
    let end = top_off + top_size;
    let (offset, size) = if end != 0 && pe.len() > end {
        (end, pe.len() - end)
    } else {
        (0, 0)
    };
    fields.push((
        "overlay",
        Node::Struct(vec![("offset", int(offset)), ("size", int(size))]),
    ));
}

fn rich(data: &[u8]) -> Node {
    use crate::sigscan::rich::{resolve_num, resolve_str};
    let path = |name: &str| format!("pe.rich_signature.{name}");
    let num = |name| resolve_num(&path(name), data).map_or(Node::Undef, int);
    let text = |name| resolve_str(&path(name), data);
    // `version_data`：解密块跳过 16 字节头之后，每条 8 字节记录的前 4 字节
    // （工具 id + 版本）依次拼起来；末尾不满一条的零头不要。
    let version_data = text("clear_data").map(|clear| {
        let (records, _) = clear.get(16..).unwrap_or(&[]).as_chunks::<8>();
        records
            .iter()
            .flat_map(|record| &record[..4])
            .copied()
            .collect()
    });
    let string = |value: Option<Vec<u8>>| value.map_or(Node::Undef, Node::Str);
    Node::Struct(vec![
        ("offset", num("offset")),
        ("length", num("length")),
        ("key", num("key")),
        ("raw_data", string(text("raw_data"))),
        ("clear_data", string(text("clear_data"))),
        ("version_data", string(version_data)),
    ])
}

/// 整棵 `pe` 对象树。官方未赋值的字段显式放 `Undef`；本实现没建模的
/// 字段不放（查不到 = 记名弃权）。
pub(super) fn build(data: &[u8]) -> Node {
    let Some(pe) = Pe::parse(data) else {
        return Node::NotPe;
    };
    let mut fields: Vec<(&'static str, Node)> = vec![("is_pe", Node::Int(1))];
    for (name, at, size) in FILE_HEADER {
        fields.push((*name, int(pe.file(*at, *size))));
    }
    for (name, at32, at64, size) in OPTIONAL {
        fields.push((*name, int(pe.opt(*at32, *at64, *size))));
    }
    let entry = pe.rva_to_offset(pe.opt(16, 16, 4));
    fields.push(("entry_point", Node::Int(entry)));
    let base_of_data = if pe.is64 {
        Node::Undef
    } else {
        int(pe.opt(24, 24, 4))
    };
    fields.push(("base_of_data", base_of_data));
    for (name, at, size) in VERSIONS {
        let pair = vec![
            ("major", int(pe.opt(*at, *at, *size))),
            ("minor", int(pe.opt(at + size, at + size, *size))),
        ];
        fields.push((*name, Node::Struct(pair)));
    }
    let mut directories = Vec::new();
    for entry in 0..(pe.opt(92, 108, 4) as usize).min(16) {
        let Some((va, size)) = pe.directory(entry) else {
            break;
        };
        directories.push(Node::Struct(vec![
            ("virtual_address", int(u64::from(va))),
            ("size", int(u64::from(size))),
        ]));
    }
    fields.push(("data_directories", Node::Array(directories)));
    yrres::collect(&pe, &mut fields);
    sections(&pe, &mut fields);
    fields.push(("rich_signature", rich(data)));
    yrimp::collect(&pe, &mut fields);
    yrexp::collect(&pe, &mut fields);
    yrdbg::collect(&pe, &mut fields);
    Node::Struct(fields)
}
