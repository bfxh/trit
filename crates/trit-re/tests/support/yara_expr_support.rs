//! `yara_expr*` 测试共用夹具（`#[path]` 引入）：单规则三值扫描助手，外加
//! 一个带导入表 / 导出表 / 资源树 / 版本信息的合成 PE32+ DLL。
//!
//! 合成 PE 的每个 `pe.*` 字段都用官方引擎（yara-python 4.5.4）逐字段对过：
//! 把官方 `modules_callback` 吐出的整棵对象树转成 `pe.<路径> == <官方值>`
//! 规则再喂给 trit，全部命中（见 docs/YARA-EXPR.md「对拍」一节）。
//! 测试里写死的数值因此是**官方给的**，不是照本实现抄的。
#![allow(dead_code)]

use trit_core::value::Ternary;
use trit_re::sigscan::SigScan;
use trit_re::yaraparse::{RuleOutcome, parse_rules};

/// 条件 → 单规则文本（`strings` 为空就不写串区）。
pub fn rule(strings: &str, condition: &str) -> String {
    let section = if strings.is_empty() {
        String::new()
    } else {
        format!("strings: {strings} ")
    };
    format!("import \"pe\"\nrule r {{ {section}condition: {condition} }}\n")
}

/// 解析单规则：`Ok(规则)` 或 `Err(Unsupported 原因)`。
pub fn parse(strings: &str, condition: &str) -> Result<trit_re::sigscan::Rule, String> {
    let out = parse_rules(&rule(strings, condition)).expect("文件级解析成功");
    assert_eq!(out.rules.len(), 1, "单规则");
    match &out.rules[0] {
        RuleOutcome::Parsed(rule) => Ok(rule.clone()),
        RuleOutcome::Unsupported { reason, .. } => Err(reason.clone()),
    }
}

/// 无模块上下文的扫描（规则必须可解析）。
pub fn eval(strings: &str, condition: &str, data: &[u8]) -> Ternary {
    let rule = parse(strings, condition).unwrap_or_else(|why| panic!("{condition}：{why}"));
    SigScan::new(std::slice::from_ref(&rule))
        .scan(data, false)
        .rules[0]
        .value
}

/// 无串规则在默认样本上的裁决。
pub fn cond(condition: &str) -> Ternary {
    eval("", condition, SAMPLE)
}

/// 带 PE 上下文的扫描（上下文由 `trit_re::pe::parse` 现算，同 CLI）。
pub fn eval_pe(condition: &str, data: &[u8]) -> Ternary {
    let rule = parse("", condition).unwrap_or_else(|why| panic!("{condition}：{why}"));
    let info = trit_re::pe::parse(data).ok();
    let entry = info
        .as_ref()
        .and_then(|pe| pe.rva_to_offset(pe.entry_rva))
        .map(|offset| offset as u64);
    SigScan::new(std::slice::from_ref(&rule))
        .scan_with_context(data, false, entry, info.as_ref())
        .rules[0]
        .value
}

/// 规则应被拒绝：返回原因。
pub fn rejected(condition: &str) -> String {
    match parse("", condition) {
        Ok(_) => panic!("`{condition}` 应为 Unsupported，却解析通过了"),
        Err(reason) => reason,
    }
}

/// 官方探针用的同一段默认样本：`hello world\0\xff`（13 字节）。
pub const SAMPLE: &[u8] = b"hello world\x00\xff";

fn put16(buf: &mut [u8], at: usize, value: u16) {
    buf[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(buf: &mut [u8], at: usize, value: u64) {
    buf[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn align4(blob: &mut Vec<u8>) {
    while !blob.len().is_multiple_of(4) {
        blob.push(0);
    }
}

/// 版本资源的一个节点：`wLength wValueLength wType szKey 对齐 值 对齐 子节点`。
fn version_node(
    key: &str,
    value: &[u8],
    value_len: u16,
    text: bool,
    children: &[Vec<u8>],
) -> Vec<u8> {
    let mut node = vec![0u8; 6];
    node.extend(utf16(key));
    node.extend([0, 0]);
    align4(&mut node);
    node.extend(value);
    align4(&mut node);
    for child in children {
        node.extend(child);
        align4(&mut node);
    }
    let length = node.len() as u16;
    put16(&mut node, 0, length);
    put16(&mut node, 2, value_len);
    put16(&mut node, 4, u16::from(text));
    node
}

fn version_string(key: &str, value: &str) -> Vec<u8> {
    let mut bytes = utf16(value);
    bytes.extend([0, 0]);
    version_node(key, &bytes, (bytes.len() / 2) as u16, true, &[])
}

/// `VS_VERSIONINFO`：固定信息 + 一张 `040904B0` 字符串表。
fn version_blob() -> Vec<u8> {
    let mut fixed = vec![0u8; 52];
    put32(&mut fixed, 0, 0xFEEF_04BD);
    put32(&mut fixed, 4, 0x0001_0000);
    let strings = [
        version_string("CompanyName", "Trit Test Works"),
        version_string("FileVersion", "1.2.3"),
        version_string("OriginalFilename", "demo.dll"),
    ];
    let table = version_node("040904B0", &[], 0, true, &strings);
    let info = version_node("StringFileInfo", &[], 0, true, &[table]);
    version_node("VS_VERSION_INFO", &fixed, 52, false, &[info])
}

const RDATA_RVA: u32 = 0x2000;
const RDATA_RAW: usize = 0x600;
const RSRC_RVA: u32 = 0x3000;
const RSRC_RAW: usize = 0xA00;

/// 合成 PE32+ DLL（0xE10 字节）：
/// - 3 节：`.text`（RVA 0x1000）、`.rdata`（0x2000）、`.rsrc`（0x3000），入口 0x1010；
/// - 导入：`KERNEL32.dll`（`GetLastError`、`ExitProcess`）+ `WS2_32.dll`（序号 115）；
/// - 导出（库名 `demo.dll`，序号基 1）：`alpha` → 0x1000、无名的序号 2 →
///   0x1008、`beta` → 转发到 `OTHER.target`；
/// - 调试目录：一条非 CodeView 项 + 一条 RSDS（PDB 路径 `demo.pdb`）；
/// - 资源：RCDATA（命名项 `CFG`，16 字节）+ VERSION（3 条字符串）；
/// - 文件尾 16 字节 overlay。
pub fn rich_pe() -> Vec<u8> {
    let mut b = vec![0u8; 0xE10];
    put(&mut b, 0, b"MZ");
    put32(&mut b, 0x3c, 0x80);
    put(&mut b, 0x80, b"PE\0\0");
    let coff = 0x84;
    put16(&mut b, coff, 0x8664);
    put16(&mut b, coff + 2, 3); // 节数
    put32(&mut b, coff + 4, 0x6543_2100); // 时间戳
    put16(&mut b, coff + 16, 0xF0); // 可选头大小
    put16(&mut b, coff + 18, 0x2022); // DLL | 可执行 | 大地址
    let opt = coff + 20;
    put16(&mut b, opt, 0x20b);
    b[opt + 2] = 14; // 链接器 14.29
    b[opt + 3] = 29;
    put32(&mut b, opt + 4, 0x200); // SizeOfCode
    put32(&mut b, opt + 8, 0x800); // SizeOfInitializedData
    put32(&mut b, opt + 16, 0x1010); // 入口 RVA
    put32(&mut b, opt + 20, 0x1000); // BaseOfCode
    put64(&mut b, opt + 24, 0x1_8000_0000); // ImageBase
    put32(&mut b, opt + 32, 0x1000); // 节对齐
    put32(&mut b, opt + 36, 0x200); // 文件对齐
    put16(&mut b, opt + 40, 6); // OS 6.1
    put16(&mut b, opt + 42, 1);
    put16(&mut b, opt + 44, 3); // 映像 3.4
    put16(&mut b, opt + 46, 4);
    put16(&mut b, opt + 48, 6); // 子系统 6.0
    put32(&mut b, opt + 56, 0x4000); // SizeOfImage
    put32(&mut b, opt + 60, 0x400); // SizeOfHeaders
    put16(&mut b, opt + 68, 3); // 控制台子系统
    put16(&mut b, opt + 70, 0x8160); // DllCharacteristics
    put64(&mut b, opt + 72, 0x10_0000);
    put64(&mut b, opt + 80, 0x1000);
    put64(&mut b, opt + 88, 0x10_0000);
    put64(&mut b, opt + 96, 0x1000);
    put32(&mut b, opt + 108, 16); // 数据目录数
    let dirs = opt + 112;
    put32(&mut b, dirs, RDATA_RVA + 0x100); // 导出目录
    put32(&mut b, dirs + 4, 0x80);
    put32(&mut b, dirs + 8, RDATA_RVA); // 导入目录
    put32(&mut b, dirs + 12, 60);
    put32(&mut b, dirs + 16, RSRC_RVA); // 资源目录
    put32(&mut b, dirs + 20, 0x400);
    put32(&mut b, dirs + 48, RDATA_RVA + 0x200); // 调试目录（两项）
    put32(&mut b, dirs + 52, 56);

    let table = opt + 0xF0;
    // (名字, [虚拟大小, RVA, 文件大小, 文件偏移, 属性])
    let sections: [(&[u8], [u32; 5]); 3] = [
        (b".text", [0x1F0, 0x1000, 0x200, 0x400, 0x6000_0020]),
        (
            b".rdata",
            [0x180, RDATA_RVA, 0x400, RDATA_RAW as u32, 0x4000_0040],
        ),
        (
            b".rsrc",
            [0x400, RSRC_RVA, 0x400, RSRC_RAW as u32, 0x4000_0040],
        ),
    ];
    for (index, (name, [vsize, rva, raw_size, raw, flags])) in sections.into_iter().enumerate() {
        let at = table + index * 40;
        put(&mut b, at, name);
        put32(&mut b, at + 8, vsize);
        put32(&mut b, at + 12, rva);
        put32(&mut b, at + 16, raw_size);
        put32(&mut b, at + 20, raw);
        put32(&mut b, at + 36, flags);
    }
    // .text：入口处一条 ret，其余 nop
    b[0x400..0x600].fill(0x90);
    b[0x410] = 0xC3;

    // ── .rdata：导入 ──
    let rd = RDATA_RAW;
    let rva = |offset: u32| RDATA_RVA + offset;
    // 描述符：ILT、时间戳、转发链、库名、IAT
    put32(&mut b, rd, rva(0x40));
    put32(&mut b, rd + 12, rva(0xB0));
    put32(&mut b, rd + 16, rva(0x68));
    put32(&mut b, rd + 20, rva(0x58));
    put32(&mut b, rd + 32, rva(0xC0));
    put32(&mut b, rd + 36, rva(0x80));
    for thunks in [0x40, 0x68] {
        put64(&mut b, rd + thunks, u64::from(rva(0x90)));
        put64(&mut b, rd + thunks + 8, u64::from(rva(0xA0)));
    }
    for thunks in [0x58, 0x80] {
        put64(&mut b, rd + thunks, (1 << 63) | 115);
    }
    put(&mut b, rd + 0x92, b"GetLastError");
    put(&mut b, rd + 0xA2, b"ExitProcess");
    put(&mut b, rd + 0xB0, b"KERNEL32.dll");
    put(&mut b, rd + 0xC0, b"WS2_32.dll");

    // ── .rdata：导出 ──
    let ex = rd + 0x100;
    put32(&mut b, ex + 4, 0x6543_2100); // 时间戳
    put32(&mut b, ex + 12, rva(0x150)); // 库名
    put32(&mut b, ex + 16, 1); // 序号基
    put32(&mut b, ex + 20, 3); // 函数数
    put32(&mut b, ex + 24, 2); // 名字数
    put32(&mut b, ex + 28, rva(0x130));
    put32(&mut b, ex + 32, rva(0x13C));
    put32(&mut b, ex + 36, rva(0x144));
    put32(&mut b, rd + 0x130, 0x1000);
    put32(&mut b, rd + 0x134, 0x1008);
    put32(&mut b, rd + 0x138, rva(0x170)); // 落在导出目录内 = 转发
    put32(&mut b, rd + 0x13C, rva(0x160));
    put32(&mut b, rd + 0x140, rva(0x168));
    put16(&mut b, rd + 0x144, 0);
    put16(&mut b, rd + 0x146, 2);
    put(&mut b, rd + 0x150, b"demo.dll");
    put(&mut b, rd + 0x160, b"alpha");
    put(&mut b, rd + 0x168, b"beta");
    put(&mut b, rd + 0x170, b"OTHER.target");

    // ── .rdata：调试目录（第一项不是 CodeView，第二项 RSDS）──
    let dbg = rd + 0x200;
    put32(&mut b, dbg + 12, 13); // 类型 13：跳过
    put32(&mut b, dbg + 28 + 12, 2); // 类型 2 = CodeView
    put32(&mut b, dbg + 28 + 16, 24 + 9);
    put32(&mut b, dbg + 28 + 20, rva(0x240));
    put32(&mut b, dbg + 28 + 24, (rd + 0x240) as u32);
    put(&mut b, rd + 0x240, b"RSDS");
    put32(&mut b, rd + 0x240 + 20, 1); // age
    put(&mut b, rd + 0x240 + 24, b"demo.pdb");

    // ── .rsrc：类型 → 名字 → 语言 三层目录 ──
    let rs = RSRC_RAW;
    let dir = |b: &mut [u8], at: usize, named: u16, ids: u16| {
        put16(b, rs + at + 12, named);
        put16(b, rs + at + 14, ids);
    };
    const SUBDIR: u32 = 0x8000_0000;
    dir(&mut b, 0x00, 0, 2); // 根：RCDATA(10)、VERSION(16)
    put32(&mut b, rs + 0x10, 10);
    put32(&mut b, rs + 0x14, SUBDIR | 0x20);
    put32(&mut b, rs + 0x18, 16);
    put32(&mut b, rs + 0x1C, SUBDIR | 0x50);
    dir(&mut b, 0x20, 1, 0); // RCDATA → 命名项 "CFG"
    put32(&mut b, rs + 0x30, SUBDIR | 0xA0);
    put32(&mut b, rs + 0x34, SUBDIR | 0x38);
    dir(&mut b, 0x38, 0, 1); // 语言 0x409
    put32(&mut b, rs + 0x48, 0x409);
    put32(&mut b, rs + 0x4C, 0x80);
    dir(&mut b, 0x50, 0, 1); // VERSION → id 1
    put32(&mut b, rs + 0x60, 1);
    put32(&mut b, rs + 0x64, SUBDIR | 0x68);
    dir(&mut b, 0x68, 0, 1); // 语言 0x409
    put32(&mut b, rs + 0x78, 0x409);
    put32(&mut b, rs + 0x7C, 0x90);
    let version = version_blob();
    put32(&mut b, rs + 0x80, RSRC_RVA + 0xB0); // RCDATA 数据项
    put32(&mut b, rs + 0x84, 16);
    put32(&mut b, rs + 0x90, RSRC_RVA + 0xC0); // VERSION 数据项
    put32(&mut b, rs + 0x94, version.len() as u32);
    put16(&mut b, rs + 0xA0, 3);
    put(&mut b, rs + 0xA2, &utf16("CFG"));
    put(&mut b, rs + 0xB0, b"config-payload!!");
    put(&mut b, rs + 0xC0, &version);

    put(&mut b, 0xE00, b"OVERLAY-16-BYTES");
    b
}
