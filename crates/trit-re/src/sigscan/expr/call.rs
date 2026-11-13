//! `pe` 模块函数（照 `pe.c` 的 `define_function` 逐个镜像）。约定：
//! - 接收者不是 PE（或已未定义）→ 未定义；任一实参未定义 → 未定义
//!   （官方 `OP_CALL`）；实参弃权 → 弃权。
//! - 没建模的函数（`imphash` / `calculate_checksum` / `locale` /
//!   `language` / `delayed_import_rva` / rich 的 `version` / `toolid` …）
//!   与需要延迟导入表的 `IMPORT_DELAYED` 分支 → 记名弃权。
//! - 官方把字符串实参当 C 串用（NUL 截断）——含 NUL 的实参弃权，不模拟。
//! - `pe.imports(/re/, /re/)` 官方要求匹配长度 > 0；现有正则引擎只报
//!   起点不报长度，所以**可匹配空串**的正则在此弃权。

use super::eval::{Env, V};
use super::node::Node;
use super::yrpe::Pe;
use super::{Arg, Matcher, strfunc};

enum A<'m> {
    Int(i64),
    Str(Vec<u8>),
    Re(&'m Matcher),
}

fn flag<'a>(value: bool) -> V<'a> {
    V::Int(i64::from(value))
}

/// 官方 `yr_re_match(...) != -1`：串里任意位置有匹配（空匹配也算）。
fn re_any(re: &Matcher, text: &[u8]) -> Option<bool> {
    match strfunc::matches(re, text) {
        V::Bool(hit) => Some(hit),
        _ => None,
    }
}

/// 官方 `yr_re_match(...) > 0`：要求非空匹配。能匹配空串的正则无从
/// 判断长度 → `None`（弃权）。
fn re_nonempty(re: &Matcher, text: &[u8]) -> Option<bool> {
    match re_any(re, b"") {
        Some(false) => re_any(re, text),
        _ => None,
    }
}

fn c_str(text: &[u8]) -> &[u8] {
    &text[..text.iter().position(|b| *b == 0).unwrap_or(text.len())]
}

/// 导入查询。`pick` 对每个（DLL 名匹配的）函数给出 是/否/不知道。
fn imports<'a>(
    root: &Node,
    dll_match: impl Fn(&[u8]) -> Option<bool>,
    pick: impl Fn(&Node) -> Option<bool>,
    count: bool,
) -> V<'a> {
    let (mut found, mut unsure) = (0i64, false);
    for dll in root.items("import_details") {
        match dll.bytes("library_name").and_then(&dll_match) {
            Some(true) => {}
            Some(false) => continue,
            None => {
                unsure = true;
                continue;
            }
        }
        for function in dll.items("functions") {
            match pick(function) {
                Some(true) => found += 1,
                Some(false) => {}
                None => unsure = true,
            }
        }
    }
    match (count, found, unsure) {
        (false, 1.., _) => V::Int(1),
        (_, _, true) => V::Unknown,
        (true, n, false) => V::Int(n),
        (false, _, false) => V::Int(0),
    }
}

/// `pe.import_rva(dll, 名字 | 序号)`：按导入表顺序第一个对上的函数的 IAT
/// RVA；DLL 名与函数名都不分大小写（官方 `ss_icompare`），找不到 = 未定义。
/// 排在前面的条目读不出确定值时不知道谁先对上 → 弃权（防御性；现有对象
/// 树里函数名总是确定的）。
fn import_rva<'a>(root: &Node, dll: &[u8], pick: impl Fn(&Node) -> Option<bool>) -> V<'a> {
    let mut unsure = false;
    for library in root.items("import_details") {
        match library.bytes("library_name") {
            Some(name) if name.eq_ignore_ascii_case(dll) => {}
            Some(_) => continue,
            None => {
                unsure = true;
                continue;
            }
        }
        for function in library.items("functions") {
            match (pick(function), function.field("rva")) {
                (Some(false), _) => {}
                (Some(true), Some(Node::Int(rva))) if !unsure => return V::Int(*rva),
                (Some(true), Some(Node::Undef)) if !unsure => return V::Undef,
                _ => unsure = true,
            }
        }
    }
    if unsure { V::Unknown } else { V::Undef }
}

fn imports_call<'a>(root: &Node, args: &[A]) -> V<'a> {
    let name_is = |want: &[u8]| {
        let want = want.to_vec();
        move |function: &Node| match function.field("name") {
            Some(Node::Str(name)) => Some(name.eq_ignore_ascii_case(&want)),
            Some(Node::Undef) => Some(false),
            _ => None,
        }
    };
    let dll_is = |want: &[u8]| {
        let want = want.to_vec();
        move |name: &[u8]| Some(name.eq_ignore_ascii_case(&want))
    };
    match args {
        [A::Str(dll), A::Str(function)] => imports(root, dll_is(dll), name_is(function), false),
        [A::Str(dll)] => imports(root, dll_is(dll), |_| Some(true), true),
        [A::Str(dll), A::Int(ordinal)] => {
            let pick = |function: &Node| Some(function.int("ordinal") == Some(*ordinal));
            imports(root, dll_is(dll), pick, false)
        }
        [A::Re(dll), A::Re(function)] => {
            let pick = |node: &Node| match node.field("name") {
                Some(Node::Str(name)) => re_nonempty(function, name),
                _ => None,
            };
            imports(root, |name| re_nonempty(dll, name), pick, true)
        }
        _ => V::Unknown,
    }
}

fn export_names(root: &Node) -> impl Iterator<Item = (usize, &[u8])> {
    let details = root.items("export_details").iter().enumerate();
    details.filter_map(|(index, node)| node.bytes("name").map(|name| (index, name)))
}

/// `exports_index(...)`：命中下标；`exports(...)` 取其"有没有"。
fn export_index(root: &Node, arg: &A) -> Result<Option<usize>, ()> {
    let details = root.items("export_details");
    match arg {
        A::Str(want) => Ok(export_names(root)
            .find(|(_, name)| name.eq_ignore_ascii_case(want))
            .map(|(index, _)| index)),
        A::Re(re) => {
            for (index, name) in export_names(root) {
                if re_any(re, c_str(name)).ok_or(())? {
                    return Ok(Some(index));
                }
            }
            Ok(None)
        }
        A::Int(ordinal) if *ordinal == 0 || *ordinal > details.len() as i64 => Ok(None),
        A::Int(ordinal) => Ok(details
            .iter()
            .position(|d| d.int("ordinal") == Some(*ordinal))),
    }
}

fn section_index<'a>(root: &Node, arg: &A) -> V<'a> {
    let Some(declared) = root.int("number_of_sections") else {
        return V::Unknown;
    };
    let sections = root.items("sections");
    for (index, section) in sections.iter().enumerate() {
        let hit = match arg {
            A::Str(want) => section.bytes("name").map(|name| c_str(name) == c_str(want)),
            A::Int(addr) => section
                .int("raw_data_offset")
                .zip(section.int("raw_data_size"))
                .map(|(start, size)| *addr >= start && *addr < start + size),
            A::Re(_) => None,
        };
        match hit {
            Some(true) => return V::Int(index as i64),
            Some(false) => {}
            None => return V::Unknown,
        }
    }
    // 节表没读全（头在文件外）时官方拿未赋值的槽位继续比——不模拟
    if sections.len() as i64 == declared.min(96) || matches!(arg, A::Str(_)) {
        V::Undef
    } else {
        V::Unknown
    }
}

pub(super) fn invoke<'a>(env: &mut Env<'a>, holder: &V<'a>, name: &str, args: &[Arg]) -> V<'a> {
    let root = match holder {
        V::Obj(Node::NotPe) | V::Undef => return V::Undef,
        V::Obj(node @ Node::Struct(_)) => *node,
        _ => return V::Unknown,
    };
    let (mut values, mut undefined, mut unknown) = (Vec::new(), false, false);
    for arg in args {
        match arg {
            Arg::Re(re) => values.push(A::Re(re)),
            Arg::Val(expr) => match env.eval(expr) {
                V::Int(value) => values.push(A::Int(value)),
                V::Str(value) if value.contains(&0) => unknown = true,
                V::Str(value) => values.push(A::Str(value)),
                V::Undef => undefined = true,
                _ => unknown = true,
            },
        }
    }
    if undefined {
        return V::Undef;
    }
    if unknown {
        return V::Unknown;
    }
    // 按要扫的表项数加收预算（循环里反复查导入 / 导出表不能白跑）；正则
    // 另按实际步数计（`metered`）
    let scanned = match name {
        "imports" | "import_rva" => root
            .items("import_details")
            .iter()
            .map(|dll| 1 + dll.items("functions").len())
            .sum(),
        "exports" | "exports_index" => root.items("export_details").len(),
        _ => 0,
    };
    if !env.burn(scanned as u64) {
        return V::Unknown;
    }
    env.metered(|env| dispatch(env, root, name, &values))
}

fn dispatch<'a>(env: &Env<'a>, root: &Node, name: &str, values: &[A]) -> V<'a> {
    let is64 = root.int("opthdr_magic").map(|magic| magic == 0x20b);
    match (name, values) {
        // 官方返回的是掩码后的值（0x2000），不是 1
        ("is_dll", []) => root
            .int("characteristics")
            .map_or(V::Unknown, |c| V::Int(c & 0x2000)),
        ("is_32bit", []) => is64.map_or(V::Unknown, |wide| flag(!wide)),
        ("is_64bit", []) => is64.map_or(V::Unknown, flag),
        ("rva_to_offset", [A::Int(rva)]) => match Pe::parse(env.pe.data()) {
            Some(pe) => match pe.rva_to_offset(*rva as u64) {
                -1 => V::Undef,
                offset => V::Int(offset),
            },
            None => V::Unknown,
        },
        ("section_index", [arg]) => section_index(root, arg),
        ("exports", [arg]) => match export_index(root, arg) {
            Ok(found) => flag(found.is_some()),
            Err(()) => V::Unknown,
        },
        ("exports_index", [arg]) => match export_index(root, arg) {
            Ok(found) => found.map_or(V::Undef, |index| V::Int(index as i64)),
            Err(()) => V::Unknown,
        },
        ("imports", [A::Int(flags), rest @ ..]) => {
            let standard = if flags & 1 != 0 {
                imports_call(root, rest)
            } else {
                V::Int(0)
            };
            let counting = matches!(rest, [A::Str(_)] | [A::Re(_), A::Re(_)]);
            match standard {
                V::Int(1) if !counting => V::Int(1),
                _ if flags & 2 != 0 => V::Unknown, // 延迟导入表未建模
                other => other,
            }
        }
        ("imports", rest) => imports_call(root, rest),
        ("import_rva", [A::Str(dll), A::Str(function)]) => {
            import_rva(root, dll, |node| match node.field("name") {
                Some(Node::Str(name)) => Some(name.eq_ignore_ascii_case(function)),
                Some(Node::Undef) => Some(false),
                _ => None,
            })
        }
        ("import_rva", [A::Str(dll), A::Int(ordinal)]) => {
            import_rva(root, dll, |node| match node.field("ordinal") {
                Some(Node::Int(value)) => Some(value == ordinal),
                Some(Node::Undef) => Some(false),
                _ => None,
            })
        }
        _ => V::Unknown,
    }
}
