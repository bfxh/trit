//! 导入表（照 `pe_parse_imports` / `pe_parse_import_descriptor` 镜像）：
//! 非法 DLL 名整项跳过、非法函数名单项跳过、没有任何函数的 DLL 不计数、
//! 每 DLL 与全局各有 16384 上限。
//!
//! 按序号导入的函数官方会合成一个名字（winsock / oleaut32 查内置序号表，
//! 其余 `ord<N>`）——见 [`super::ordnames`]。如实弃权：延迟导入表未建模
//! （`delayed_*` 字段记名弃权）。

use super::node::Node;
use super::yrpe::Pe;

const MAX_PE_IMPORTS: usize = 16384;
const MAX_DLL_NAME: usize = 256;

fn int(value: u64) -> Node {
    Node::Int(value as i64)
}

fn valid_dll_name(name: &[u8], limit: usize) -> bool {
    let ok = |c: &u8| (b' '..=0x7e).contains(c) && !b"\"*<>?|".contains(c);
    !name.is_empty() && name.len() < limit && name.iter().all(ok)
}

fn valid_function_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._?@$()<>".contains(c))
}

/// 一个导入描述符下的函数表。`total` 是跨 DLL 的累计函数数。
fn functions(pe: &Pe, descriptor: u64, dll: &[u8], total: &mut usize) -> Vec<Node> {
    let word = |field: u64| u64::from(pe.u32(descriptor + field).unwrap_or(0));
    let first_thunk = word(16);
    let mut offset = pe.rva_to_offset(word(0));
    if offset <= 0 {
        offset = pe.rva_to_offset(first_thunk);
    }
    let mut out = Vec::new();
    if offset < 0 {
        return out;
    }
    let (width, flag, max_ordinal) = if pe.is64 {
        (8u64, 1u64 << 63, 0x8000_0000_0000_ffffu64)
    } else {
        (4u64, 1u64 << 31, 0x8000_ffffu64)
    };
    let mut thunk = offset as u64;
    let (mut parsed, mut index) = (0usize, 0u64);
    while pe.fits(thunk, width) && parsed < MAX_PE_IMPORTS && *total < MAX_PE_IMPORTS {
        let value = if pe.is64 {
            pe.u64(thunk).unwrap_or(0)
        } else {
            u64::from(pe.u32(thunk).unwrap_or(0))
        };
        if value == 0 {
            break;
        }
        parsed += 1;
        let (mut name, mut ordinal) = (None, None);
        if value & flag == 0 {
            let at = pe.rva_to_offset(value);
            // IMAGE_IMPORT_BY_NAME：2 字节 Hint + 名字（结构体尺寸 4）
            if at >= 0 && pe.fits(at as u64, 4) {
                name = Some(Node::Str(pe.cstr(at as u64 + 2, 512).to_vec()));
            }
        } else if value <= max_ordinal {
            let number = (value & 0xffff) as u16;
            ordinal = Some(number);
            name = Some(Node::Str(super::ordnames::name(dll, number)));
        }
        let rva = first_thunk + width * index;
        thunk += width;
        index += 1;
        if matches!(&name, Some(Node::Str(n)) if !valid_function_name(n)) {
            continue;
        }
        if name.is_some() || ordinal.is_some() {
            out.push(Node::Struct(vec![
                ("name", name.unwrap_or(Node::Undef)),
                (
                    "ordinal",
                    ordinal.map_or(Node::Undef, |o| int(u64::from(o))),
                ),
                ("rva", if rva == 0 { Node::Undef } else { int(rva) }),
            ]));
            *total += 1;
        }
    }
    out
}

fn parse(pe: &Pe) -> Option<(Vec<Node>, usize)> {
    let (dir_va, _) = pe.directory(1)?;
    if dir_va == 0 {
        return None;
    }
    let offset = pe.rva_to_offset(u64::from(dir_va));
    if offset < 0 {
        return None;
    }
    let mut descriptor = offset as u64;
    let (mut dlls, mut total, mut parsed) = (Vec::new(), 0usize, 0usize);
    while pe.fits(descriptor, 20) && parsed < MAX_PE_IMPORTS {
        let name_rva = u64::from(pe.u32(descriptor + 12).unwrap_or(0));
        if name_rva == 0 {
            break;
        }
        parsed += 1;
        let current = descriptor;
        descriptor += 20;
        let at = pe.rva_to_offset(name_rva);
        if at < 0 {
            continue;
        }
        let limit = ((pe.len() - at as u64) as usize).min(MAX_DLL_NAME);
        let name = pe.cstr(at as u64, limit);
        if !valid_dll_name(name, limit) {
            continue;
        }
        let funcs = functions(pe, current, name, &mut total);
        if funcs.is_empty() {
            continue;
        }
        dlls.push(Node::Struct(vec![
            ("library_name", Node::Str(name.to_vec())),
            ("number_of_functions", int(funcs.len() as u64)),
            ("functions", Node::Array(funcs)),
        ]));
    }
    Some((dlls, total))
}

pub(super) fn collect(pe: &Pe, fields: &mut Vec<(&'static str, Node)>) {
    let (dlls, total) = parse(pe).unwrap_or_default();
    fields.push(("number_of_imports", int(dlls.len() as u64)));
    fields.push(("number_of_imported_functions", int(total as u64)));
    fields.push(("import_details", Node::Array(dlls)));
}
