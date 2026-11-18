//! 资源树与版本信息（照 `pe_iterate_resources` / `pe_collect_resources` /
//! `pe_parse_version_info` 镜像）。照搬的官方怪癖：
//! - 目录项 `OffsetToData == 0` 时官方 `continue` 却不前移指针——等价于
//!   本层到此为止；
//! - `type` / `id` / `language` 是跨整棵树共享的三个整数（叶子出现在
//!   浅层时会带着上一棵子树的旧值）；
//! - 名字是字符串时 `*_string` 存 UTF-16LE **原始字节**，对应的整数字段
//!   不赋值（未定义）；
//! - 版本信息的键 / 值只取每个 UTF-16 码元的低字节，遇低字节 0 即止，
//!   键截 63、值截 255 字节；同名键后者覆盖前者。

use super::node::Node;
use super::yrpe::Pe;

const MAX_RESOURCES: usize = 65536;
const RESOURCE_TYPE_VERSION: i32 = 16;

fn int(value: u64) -> Node {
    Node::Int(value as i64)
}

fn align4(value: u64) -> u64 {
    (value + 3) & !3
}

struct Walk<'p, 'a> {
    pe: &'p Pe<'a>,
    /// 资源节起点（所有目录内偏移相对它）。
    base: u64,
    kind: i32,
    id: i32,
    language: i32,
    resources: Vec<Node>,
    versions: Vec<(Vec<u8>, Vec<u8>)>,
}

/// 三层各自的名字串位置（`None` = 该层用整数 ID）。
type Names = [Option<u64>; 3];

impl Walk<'_, '_> {
    fn name_string(&self, name: u32) -> Option<u64> {
        if name & 0x8000_0000 == 0 {
            return None;
        }
        let at = self.base + u64::from(name & 0x7fff_ffff);
        if !self.pe.fits(at, 2) {
            return None;
        }
        let length = u64::from(self.pe.u16(at)?);
        (length <= 1000 && self.pe.fits(at, 2 + length * 2)).then_some(at)
    }

    fn directory(&mut self, dir: u64, level: usize, mut names: Names) {
        let pe = self.pe;
        let short = |field: u64| u64::from(pe.u16(dir + field).unwrap_or(0));
        let (named, ids) = (short(12), short(14));
        if pe.u32(dir).unwrap_or(0) != 0 || named > 32768 || ids > 32768 {
            return;
        }
        let total = named + ids;
        if !pe.fits(dir + 16, total * 8) {
            return;
        }
        for index in 0..total {
            let entry = dir + 16 + index * 8;
            let name = pe.u32(entry).unwrap_or(0);
            let target = pe.u32(entry + 4).unwrap_or(0);
            if target == 0 {
                return;
            }
            let slot = [&mut self.kind, &mut self.id, &mut self.language];
            *slot[level] = name as i32;
            names[level] = self.name_string(name);
            let at = self.base + u64::from(target & 0x7fff_ffff);
            if target & 0x8000_0000 != 0 && level < 2 {
                if pe.fits(at, 16) {
                    self.directory(at, level + 1, names);
                }
            } else if pe.fits(at, 16) {
                let size = pe.u32(at + 4).unwrap_or(0);
                if size > 0 && size <= 0x3FFF_FFFF {
                    self.leaf(at, size, names);
                }
            }
        }
    }

    /// 字符串名 → `*_string` 有值、整数未定义；整数名 → 反之（-1 = 没设过）。
    fn name_pair(&self, at: Option<u64>, id: i32) -> (Node, Node) {
        let Some(at) = at else {
            let number = if id == -1 {
                Node::Undef
            } else {
                Node::Int(i64::from(id))
            };
            return (number, Node::Undef);
        };
        let length = u64::from(self.pe.u16(at).unwrap_or(0)) * 2;
        let text = if self.pe.fits(at + 2, length) {
            let start = at as usize + 2;
            Node::Str(self.pe.d[start..start + length as usize].to_vec())
        } else {
            Node::Undef
        };
        (Node::Undef, text)
    }

    fn leaf(&mut self, entry: u64, size: u32, names: Names) {
        if self.resources.len() >= MAX_RESOURCES {
            return;
        }
        let rva = u64::from(self.pe.u32(entry).unwrap_or(0));
        let offset = self.pe.rva_to_offset(rva);
        let (kind, kind_string) = self.name_pair(names[0], self.kind);
        let (id, name_string) = self.name_pair(names[1], self.id);
        let (language, language_string) = self.name_pair(names[2], self.language);
        self.resources.push(Node::Struct(vec![
            ("rva", int(rva)),
            (
                "offset",
                if offset < 0 {
                    Node::Undef
                } else {
                    Node::Int(offset)
                },
            ),
            ("length", int(u64::from(size))),
            ("type", kind),
            ("type_string", kind_string),
            ("id", id),
            ("name_string", name_string),
            ("language", language),
            ("language_string", language_string),
        ]));
        if self.kind == RESOURCE_TYPE_VERSION && offset >= 0 {
            self.version_info(offset as u64);
        }
    }

    /// `strcmp_w(w, ascii) == 0`：逐码元比对且以宽 NUL 结尾。
    fn wide_eq(&self, at: u64, ascii: &[u8]) -> bool {
        let unit = |i: usize| self.pe.u16(at + 2 * i as u64);
        ascii
            .iter()
            .enumerate()
            .all(|(i, c)| unit(i) == Some(u16::from(*c)))
            && unit(ascii.len()) == Some(0)
    }

    /// `wide_string_fits_in_pe` + `strnlen_w`：宽 NUL 之前的码元数。
    fn wide_len(&self, at: u64) -> Option<u64> {
        (0..)
            .map_while(|i| self.pe.u16(at + 2 * i))
            .position(|unit| unit == 0)
            .map(|n| n as u64)
    }

    /// `strlcpy_w`：取每个码元的低字节，遇 0 或写满 `limit - 1` 即止。
    fn narrow(&self, at: u64, limit: usize) -> Vec<u8> {
        (0..limit as u64 - 1)
            .map_while(|i| self.pe.d.get((at + 2 * i) as usize).copied())
            .take_while(|byte| *byte != 0)
            .collect()
    }

    fn key_fits(&self, at: u64, ascii: &[u8]) -> bool {
        self.pe.fits(at + 6, (ascii.len() as u64 + 1) * 2) && self.wide_eq(at + 6, ascii)
    }

    fn version_info(&mut self, start: u64) {
        let pe = self.pe;
        let length = |at: u64| u64::from(pe.u16(at).unwrap_or(0));
        if !pe.fits(start, 6) || !self.key_fits(start, b"VS_VERSION_INFO") {
            return;
        }
        let mut info = start + align4(6 + 86);
        while self.key_fits(info, b"VarFileInfo") && length(info) != 0 {
            info += align4(length(info));
        }
        while self.key_fits(info, b"StringFileInfo") && length(info) != 0 {
            let mut table = info + align4(6 + 30);
            info += align4(length(info));
            while pe.fits(table, 6) && length(table) != 0 && table < info {
                let Some(key_len) = self.wide_len(table + 6) else {
                    break;
                };
                let mut string = table + align4(6 + 2 * (key_len + 1));
                table += align4(length(table));
                while pe.fits(string, 6) && length(string) != 0 && string < table {
                    let Some(key_len) = self.wide_len(string + 6) else {
                        break;
                    };
                    let value_at = string + align4(6 + 2 * (key_len + 1));
                    if self.wide_len(value_at).is_some() {
                        let key = self.narrow(string + 6, 64);
                        let empty = pe.u16(string + 2) == Some(0);
                        let value = if empty {
                            Vec::new()
                        } else {
                            self.narrow(value_at, 256)
                        };
                        self.versions.push((key, value));
                    }
                    string += align4(length(string));
                }
            }
        }
    }
}

pub(super) fn collect(pe: &Pe, fields: &mut Vec<(&'static str, Node)>) {
    let mut walk = Walk {
        pe,
        base: 0,
        kind: -1,
        id: -1,
        language: -1,
        resources: Vec::new(),
        versions: Vec::new(),
    };
    let mut header = None;
    if let Some((va, _)) = pe.directory(2)
        && va != 0
    {
        let offset = pe.rva_to_offset(u64::from(va));
        if offset >= 0 && pe.fits(offset as u64, 16) {
            let at = offset as u64;
            header = Some((pe.u32(at + 4), pe.u16(at + 8), pe.u16(at + 10)));
            walk.base = at;
            walk.directory(at, 0, [None; 3]);
        }
    }
    let field = |value: Option<u64>| value.map_or(Node::Undef, int);
    let (stamp, major, minor) = match header {
        Some((stamp, major, minor)) => (
            field(stamp.map(u64::from)),
            field(major.map(u64::from)),
            field(minor.map(u64::from)),
        ),
        None => (Node::Undef, Node::Undef, Node::Undef),
    };
    fields.push(("resource_timestamp", stamp));
    fields.push((
        "resource_version",
        Node::Struct(vec![("major", major), ("minor", minor)]),
    ));
    fields.push(("number_of_resources", int(walk.resources.len() as u64)));
    fields.push(("resources", Node::Array(walk.resources)));
    fields.push(("number_of_version_infos", int(walk.versions.len() as u64)));
    let list = walk.versions.iter().map(|(key, value)| {
        Node::Struct(vec![
            ("key", Node::Str(key.clone())),
            ("value", Node::Str(value.clone())),
        ])
    });
    fields.push(("version_info_list", Node::Array(list.collect())));
    let mut dict: Vec<(Vec<u8>, Node)> = Vec::new();
    for (key, value) in walk.versions {
        match dict.iter_mut().find(|(known, _)| *known == key) {
            Some((_, slot)) => *slot = Node::Str(value),
            None => dict.push((key, Node::Str(value))),
        }
    }
    fields.push(("version_info", Node::Dict(dict)));
}
