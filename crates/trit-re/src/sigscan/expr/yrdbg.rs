//! 调试目录里的 PDB 路径（照 `pe_parse_debug_directory` 镜像）：数据目录 6
//! → `IMAGE_DEBUG_DIRECTORY[]`（每项 28 字节）→ 第一条能读出路径的
//! CodeView 项。三种 CodeView 头（按 1 字节对齐的结构体尺寸 / 路径偏移）：
//! `NB10`（17 / 16）、`RSDS`（25 / 24）、`MTOC`（21 / 20）。
//!
//! 照搬的怪癖：调试数据先按 RVA 找，换算不出来（≤ 0）再把
//! `PointerToRawData` 当文件偏移用；路径读到 NUL / 文件尾为止，长度必须
//! 小于 `YR_MAX_PATH`；官方的路径指针**跨表项保留**（前一项的路径太长没
//! 落账，后一项又不是认识的 CodeView 头时，会再试一次前一项的路径——结果
//! 一样，只是别在这里"顺手修掉"）。
//!
//! `YR_MAX_PATH` 随平台变（Linux 4096，Windows 是 `MAX_PATH` = 260）；这里
//! 以对拍基准（x86-64 Linux 的 yara-python 4.5.4）为准。

use super::node::Node;
use super::yrpe::Pe;

const MAX_PATH: usize = 4096;
const ENTRY: u64 = 28;
const CODEVIEW: u32 = 2;

pub(super) fn collect(pe: &Pe, fields: &mut Vec<(&'static str, Node)>) {
    fields.push(("pdb_path", pdb_path(pe).map_or(Node::Undef, Node::Str)));
}

fn pdb_path(pe: &Pe) -> Option<Vec<u8>> {
    let (va, size) = pe.directory(6)?;
    if va == 0 || size == 0 {
        return None;
    }
    let base = u64::try_from(pe.rva_to_offset(u64::from(va))).ok()?;
    let mut name_at = None;
    for index in 0..u64::from(size) / ENTRY {
        let entry = base + index * ENTRY;
        if !pe.fits(entry, ENTRY) {
            break;
        }
        if pe.u32(entry + 12)? != CODEVIEW {
            continue;
        }
        let (rva, raw) = (pe.u32(entry + 20)?, pe.u32(entry + 24)?);
        let mut header = match rva {
            0 => 0,
            rva => pe.rva_to_offset(u64::from(rva)),
        };
        if header <= 0 && raw != 0 {
            header = i64::from(raw);
        }
        if header <= 0 || !pe.fits(header as u64, 8) {
            continue;
        }
        let header = header as u64;
        let layout = match pe.u32(header)? {
            0x3031_424E => Some((17, 16)), // "NB10"
            0x5344_5352 => Some((25, 24)), // "RSDS"
            0x434F_544D => Some((21, 20)), // "MTOC"
            _ => None,
        };
        if let Some((size, offset)) = layout
            && pe.fits(header, size)
        {
            name_at = Some(header + offset);
        }
        if let Some(at) = name_at {
            let name = pe.cstr(at, MAX_PATH);
            if name.len() < MAX_PATH {
                return Some(name.to_vec());
            }
        }
    }
    None
}
