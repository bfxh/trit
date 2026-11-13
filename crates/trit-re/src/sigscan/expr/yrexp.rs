//! 导出表（照 `pe_parse_exports` 镜像，含它的提前返回与整数怪癖）：
//! `number_of_exports` 先置 0，任何一步失败就停在已赋值的状态。

use super::node::Node;
use super::yrpe::Pe;

const MAX_PE_EXPORTS: u64 = 16384;
const MAX_EXPORT_NAME_LENGTH: usize = 512;

fn int(value: u64) -> Node {
    Node::Int(value as i64)
}

pub(super) fn collect(pe: &Pe, fields: &mut Vec<(&'static str, Node)>) {
    let mut timestamp = Node::Undef;
    let mut dll_name = Node::Undef;
    let details = parse(pe, &mut timestamp, &mut dll_name).unwrap_or_default();
    fields.push(("number_of_exports", int(details.len() as u64)));
    fields.push(("export_timestamp", timestamp));
    fields.push(("dll_name", dll_name));
    fields.push(("export_details", Node::Array(details)));
}

fn parse(pe: &Pe, timestamp: &mut Node, dll_name: &mut Node) -> Option<Vec<Node>> {
    let len = pe.len();
    let (dir_va, dir_size) = pe.directory(0)?;
    if dir_va == 0 {
        return None;
    }
    let start = pe.rva_to_offset(u64::from(dir_va));
    if start < 0 || !pe.fits(start as u64, 40) {
        return None;
    }
    let at = start as u64;
    let word = |field: u64| pe.u32(at + field).unwrap_or(0);
    let count = u64::from(word(20)).min(MAX_PE_EXPORTS);
    let base = word(16);
    *timestamp = int(u64::from(word(4)));
    let mut offset = pe.rva_to_offset(u64::from(word(12)));
    if offset > 0 {
        *dll_name = Node::Str(pe.cstr(offset as u64, usize::MAX).to_vec());
    }
    // 官方：`number_of_exports * 4 > data_size - offset`（offset 可为 -1，
    // 无符号回绕）——照搬
    if count * 4 > len.wrapping_sub(offset as u64) {
        return None;
    }
    let name_count = u64::from(word(24));
    let mut names = None;
    if name_count > 0 {
        offset = pe.rva_to_offset(u64::from(word(32)));
        if offset < 0 || name_count * 4 > len.wrapping_sub(offset as u64) {
            return None;
        }
        names = Some(offset as u64);
    }
    let available = |at: u64| len.saturating_sub(at);
    offset = pe.rva_to_offset(u64::from(word(36)));
    if offset < 0 || available(offset as u64) < 2 * count {
        return None;
    }
    let ordinals = offset as u64;
    offset = pe.rva_to_offset(u64::from(word(28)));
    if offset < 0 || available(offset as u64) < 4 * count {
        return None;
    }
    let functions = offset as u64;
    let name_count = name_count.min(count);
    let (start, end) = (start, start + i64::from(dir_size));
    let mut details = Vec::new();
    for i in 0..count {
        let rva = pe.u32(functions + 4 * i).unwrap_or(0);
        let target = pe.rva_to_offset(u64::from(rva));
        let mut entry = vec![
            ("ordinal", int(u64::from(base.wrapping_add(i as u32)))),
            ("rva", int(u64::from(rva))),
        ];
        // 落在导出目录内的"函数地址"其实是转发串
        if target > start && target < end {
            let forward = pe.cstr(target as u64, MAX_EXPORT_NAME_LENGTH);
            entry.push(("forward_name", Node::Str(forward.to_vec())));
            entry.push(("offset", Node::Undef));
        } else {
            entry.push(("forward_name", Node::Undef));
            let offset = if target < 0 {
                Node::Undef
            } else {
                Node::Int(target)
            };
            entry.push(("offset", offset));
        }
        let mut name = Node::Undef;
        if let Some(names) = names {
            for j in 0..name_count {
                if u64::from(pe.u16(ordinals + 2 * j).unwrap_or(0)) != i {
                    continue;
                }
                let at = pe.rva_to_offset(u64::from(pe.u32(names + 4 * j).unwrap_or(0)));
                if at > 0 {
                    name = Node::Str(pe.cstr(at as u64, MAX_EXPORT_NAME_LENGTH).to_vec());
                }
            }
        }
        entry.push(("name", name));
        details.push(Node::Struct(entry));
    }
    Some(details)
}
