//! `rich_pe()` 的字节级变体，逐个对官方核过：
//!
//! - `pe.pdb_path`：调试目录的各种形状（三种 CodeView 头、RVA / 文件偏移
//!   两条取址路径、没有 NUL 结尾、路径过长、表项越界）；
//! - `pe.rich_signature.version_data`：DOS 桩里塞一段 Rich 头（整条记录 /
//!   末尾带半条记录）。
//!
//! 期望值 = 官方 yara-python 4.5.4 读同一份字节的结论。复算：
//!
//! ```text
//! TRIT_EXPR_VARIANTS=<空目录> cargo test -p trit-re --test yara_expr_variants
//! python -I scripts/yara_pe_walk.py --trit <trit-re> --samples <该目录>
//! ```
//!
//! （遍历器从官方对象树反推规则：字段有值就生成 `== "<值>"`，没值就生成
//! `not defined`，DIFF 0 即两边读出同一个结论。）

#[path = "support/yara_expr_support.rs"]
mod yara_expr_support;

use trit_core::value::Ternary;
use yara_expr_support::{eval_pe, rich_pe};

/// 夹具里的固定位置（见 `rich_pe()`）。
const DIR_SLOT: usize = 0x138; // 数据目录 6：VirtualAddress，+4 = Size
const FIRST: usize = 0x800; // 调试目录第一项（类型 13）
const SECOND: usize = 0x81C; // 第二项（CodeView）
const RECORD: usize = 0x840; // RSDS 记录，路径在 +24
const TYPE: usize = 12;
const RVA: usize = 20;
const RAW: usize = 24;

fn put(data: &mut [u8], at: usize, bytes: &[u8]) {
    data[at..at + bytes.len()].copy_from_slice(bytes);
}

fn put32(data: &mut [u8], at: usize, value: u32) {
    put(data, at, &value.to_le_bytes());
}

/// 把第一项改成 CodeView，指向文件尾新接上的一条 RSDS 记录，路径是
/// `length` 个 `A`、没有 NUL（读到文件尾为止）。
fn long_path_first(data: &mut Vec<u8>, length: usize) {
    let record = data.len();
    data.extend_from_slice(b"RSDS");
    data.extend_from_slice(&[0; 20]);
    data.extend(std::iter::repeat_n(b'A', length));
    put32(data, FIRST + TYPE, 2);
    put32(data, FIRST + RAW, record as u32);
}

type Variant = (&'static str, fn(&mut Vec<u8>), Option<&'static [u8]>);

const VARIANTS: &[Variant] = &[
    ("rsds", |_| {}, Some(b"demo.pdb")),
    (
        "nb10",
        |d| {
            put(d, RECORD, b"NB10");
            put(d, RECORD + 16, b"nb.pdb\0");
        },
        Some(b"nb.pdb"),
    ),
    (
        "mtoc",
        |d| {
            put(d, RECORD, b"MTOC");
            put(d, RECORD + 20, b"mt.pdb\0");
        },
        Some(b"mt.pdb"),
    ),
    ("unknown_signature", |d| put(d, RECORD, b"XXXX"), None),
    (
        "raw_pointer_only",
        |d| put32(d, SECOND + RVA, 0),
        Some(b"demo.pdb"),
    ),
    (
        // RVA 换算不出来 → 退回文件偏移
        "unmapped_rva_falls_back_to_raw",
        |d| put32(d, SECOND + RVA, 0x9000_0000),
        Some(b"demo.pdb"),
    ),
    (
        "no_address_at_all",
        |d| {
            put32(d, SECOND + RVA, 0);
            put32(d, SECOND + RAW, 0);
        },
        None,
    ),
    (
        // 记录贴着文件尾：路径没有 NUL，读到文件尾
        "unterminated_at_end_of_file",
        |d| {
            let record = d.len() - 28;
            put(d, record, b"RSDS");
            put(d, record + 24, b"abcd");
            put32(d, SECOND + RVA, 0);
            put32(d, SECOND + RAW, record as u32);
        },
        Some(b"abcd"),
    ),
    (
        // 头的 8 字节在文件内，整条 RSDS 记录（25 字节）不在
        "record_cut_by_end_of_file",
        |d| {
            let record = d.len() - 24;
            put(d, record, b"RSDS");
            put32(d, SECOND + RVA, 0);
            put32(d, SECOND + RAW, record as u32);
        },
        None,
    ),
    ("not_codeview", |d| put32(d, SECOND + TYPE, 13), None),
    (
        "size_covers_one_entry",
        |d| put32(d, DIR_SLOT + 4, 55),
        None,
    ),
    (
        "size_with_slack",
        |d| put32(d, DIR_SLOT + 4, 83),
        Some(b"demo.pdb"),
    ),
    ("size_zero", |d| put32(d, DIR_SLOT + 4, 0), None),
    (
        "directory_unmapped",
        |d| put32(d, DIR_SLOT, 0x9000_0000),
        None,
    ),
    (
        // 路径长度必须 < 4096：4096 个字节的不认，接着看下一项
        "too_long_then_valid",
        |d| long_path_first(d, 4096),
        Some(b"demo.pdb"),
    ),
    (
        "too_long_and_nothing_else",
        |d| {
            long_path_first(d, 4096);
            put32(d, SECOND + TYPE, 13);
        },
        None,
    ),
    (
        // 前一项的路径指针官方不清零：下一项认不出头时再试一次，结论不变
        "too_long_then_unknown_signature",
        |d| {
            long_path_first(d, 5000);
            put(d, RECORD, b"XXXX");
        },
        None,
    ),
];

fn build(mutate: fn(&mut Vec<u8>)) -> Vec<u8> {
    let mut data = rich_pe();
    mutate(&mut data);
    data
}

fn literal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\x{byte:02x}")).collect()
}

#[test]
fn pdb_path_follows_the_reference_parser_on_every_directory_shape() {
    for (name, mutate, expected) in VARIANTS {
        let data = build(*mutate);
        let (hit, miss) = match expected {
            Some(path) => (
                format!(r#"pe.pdb_path == "{}""#, literal(path)),
                "not defined pe.pdb_path".to_string(),
            ),
            None => (
                "not defined pe.pdb_path".to_string(),
                r#"pe.pdb_path matches /./ or pe.pdb_path == """#.to_string(),
            ),
        };
        assert_eq!(eval_pe(&hit, &data), Ternary::True, "{name}: {hit}");
        assert_eq!(eval_pe(&miss, &data), Ternary::False, "{name}: {miss}");
    }
}

#[test]
fn longest_accepted_path_is_4095_bytes() {
    let mut data = rich_pe();
    long_path_first(&mut data, 4095);
    let exact = format!(r#"pe.pdb_path == "{}""#, "A".repeat(4095));
    assert_eq!(eval_pe(&exact, &data), Ternary::True);
    let longer = format!(r#"pe.pdb_path == "{}""#, "A".repeat(4096));
    assert_eq!(eval_pe(&longer, &data), Ternary::False);
}

/// 在 DOS 桩（0x40 起）写一段 Rich 头：`DanS`、三个填充字、`words` 里的
/// 明文字、`Rich`、密钥；前面几项都按密钥异或。
fn with_rich_header(words: &[u32]) -> Vec<u8> {
    const KEY: u32 = 0x1122_3344;
    let mut data = rich_pe();
    let mut at = 0x40;
    for word in [0x536E_6144, 0, 0, 0].iter().chain(words) {
        put32(&mut data, at, word ^ KEY);
        at += 4;
    }
    put(&mut data, at, b"Rich");
    put32(&mut data, at + 4, KEY);
    data
}

/// 两条完整记录：(工具 0x93 版本 0x7809，3 次)、(工具 0x104 版本 0x8410，1 次)。
const RECORDS: [u32; 4] = [0x0093_7809, 3, 0x0104_8410, 1];

#[test]
fn rich_version_data_is_the_first_half_of_every_record() {
    let data = with_rich_header(&RECORDS);
    for condition in [
        "pe.rich_signature.key == 0x11223344 and pe.rich_signature.length == 32",
        r#"pe.rich_signature.version_data == "\x09\x78\x93\x00\x10\x84\x04\x01""#,
        r#"pe.rich_signature.clear_data startswith "DanS""#,
    ] {
        assert_eq!(eval_pe(condition, &data), Ternary::True, "{condition}");
    }
    // 末尾多出半条记录（只有 4 字节）：不进 version_data
    let ragged = with_rich_header(&RECORDS[..3]);
    for condition in [
        "pe.rich_signature.length == 28",
        r#"pe.rich_signature.version_data == "\x09\x78\x93\x00""#,
    ] {
        assert_eq!(eval_pe(condition, &ragged), Ternary::True, "{condition}");
    }
    // 没有 Rich 头：确定未定义，不是弃权
    let plain = rich_pe();
    let absent = "not defined pe.rich_signature.version_data";
    assert_eq!(eval_pe(absent, &plain), Ternary::True);
    assert_eq!(eval_pe(absent, &data), Ternary::False);
}

#[test]
fn variants_can_be_exported_for_the_reference_walk() {
    let mut longest = rich_pe();
    long_path_first(&mut longest, 4095);
    let mut files: Vec<(String, Vec<u8>)> = VARIANTS
        .iter()
        .map(|(name, mutate, _)| (format!("pdb_{name}.dll"), build(*mutate)))
        .collect();
    files.push(("pdb_longest_4095.dll".to_string(), longest));
    files.push((
        "rich_two_records.dll".to_string(),
        with_rich_header(&RECORDS),
    ));
    files.push((
        "rich_ragged_tail.dll".to_string(),
        with_rich_header(&RECORDS[..3]),
    ));
    assert_eq!(files.len(), VARIANTS.len() + 3);
    for (name, data) in &files {
        assert!(trit_re::pe::parse(data).is_ok(), "{name} 仍是可解析的 PE");
    }
    if let Some(root) = std::env::var_os("TRIT_EXPR_VARIANTS") {
        let root = std::path::PathBuf::from(root);
        for (name, data) in &files {
            std::fs::write(root.join(name), data).expect("写出变体");
        }
    }
}
