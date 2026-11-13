//! 表达式语言 C / E 项：`pe` 对象模型——动态下标（`pe.sections[i].name`）、
//! `pe.imports` / `pe.exports` / `pe.resources` / `pe.version_info` 的数组式
//! 访问与函数。样本是 `support/yara_expr_support.rs` 里的合成 PE32+ DLL；
//! 期望值 = 官方 yara-python 4.5.4 在同一份字节上的结论（复算：
//! `scripts/yara_expr_probe.py --pe <导出的夹具>`）。

#[path = "support/yara_expr_support.rs"]
mod yara_expr_support;

use trit_core::value::Ternary;
use yara_expr_support::{cond, eval_pe, rejected, rich_pe};

fn assert_all(table: &[&str], expected: Ternary) {
    let data = rich_pe();
    for condition in table {
        assert_eq!(eval_pe(condition, &data), expected, "{condition}");
    }
}

#[test]
fn fixture_can_be_exported_for_the_reference_probe() {
    let data = rich_pe();
    assert_eq!(data.len(), 0xE10);
    assert!(
        trit_re::pe::parse(&data).is_ok(),
        "本仓的 PE 解析器认这份夹具"
    );
    if let Some(path) = std::env::var_os("TRIT_EXPR_FIXTURE") {
        std::fs::write(path, &data).expect("写出夹具");
    }
}

// probe: sample=rich
const HEADER_HIT: &[&str] = &[
    "pe.is_pe == 1",
    "pe.machine == pe.MACHINE_AMD64 and pe.machine == 0x8664",
    "pe.number_of_sections == 3 and pe.timestamp == 0x65432100",
    "pe.characteristics == 0x2022 and pe.characteristics & pe.DLL != 0",
    "pe.is_dll() == 0x2000",
    "pe.is_dll() == pe.characteristics & pe.DLL",
    "pe.is_64bit() and not pe.is_32bit()",
    "pe.entry_point_raw == 0x1010 and pe.entry_point == 0x410",
    "pe.entry_point == entrypoint",
    "pe.rva_to_offset(pe.entry_point_raw) == pe.entry_point",
    "pe.image_base == 0x180000000",
    "pe.opthdr_magic == pe.IMAGE_NT_OPTIONAL_HDR64_MAGIC",
    "pe.linker_version.major == 14 and pe.linker_version.minor == 29",
    "pe.os_version.major * 10 + pe.os_version.minor == 61",
    "pe.image_version.major + pe.image_version.minor == 7",
    "pe.subsystem == pe.SUBSYSTEM_WINDOWS_CUI",
    "pe.dll_characteristics & pe.DYNAMIC_BASE != 0",
    "pe.dll_characteristics & pe.NX_COMPAT != 0",
    "pe.size_of_image % pe.section_alignment == 0",
    "pe.overlay.offset == 0xE00 and pe.overlay.size == 16",
    "pe.overlay.offset + pe.overlay.size == filesize",
    "pe.data_directories[pe.IMAGE_DIRECTORY_ENTRY_EXPORT].virtual_address == 0x2100",
    "pe.data_directories[pe.IMAGE_DIRECTORY_ENTRY_IMPORT].size == 60",
    "pe.data_directories[pe.IMAGE_DIRECTORY_ENTRY_RESOURCE].size == 0x400",
    "pe.dll_name == \"demo.dll\" and pe.export_timestamp == pe.timestamp",
    "not defined pe.rich_signature.key",
];

// probe: sample=rich
const SECTION_HIT: &[&str] = &[
    r#"pe.sections[0].name == ".text" and pe.sections[1].name == ".rdata""#,
    "pe.sections[2].virtual_address == 0x3000 and pe.sections[2].raw_data_offset == 0xA00",
    "pe.sections[0].virtual_size == 0x1F0 and pe.sections[0].raw_data_size == 0x200",
    "pe.sections[0].characteristics & pe.SECTION_MEM_EXECUTE != 0",
    // 动态下标：下标是任意整数表达式
    r#"pe.sections[pe.number_of_sections - 1].name == ".rsrc""#,
    r#"pe.sections[filesize - filesize + 1].name == ".rdata""#,
    r#"pe.sections[pe.section_index(".rsrc")].virtual_address == 0x3000"#,
    r#"for any i in (0..pe.number_of_sections - 1) : (pe.sections[i].name == ".rsrc")"#,
    "for all i in (0..pe.number_of_sections - 1) : (defined pe.sections[i].name)",
    "for all i in (0..pe.number_of_sections - 1) : \
     (pe.sections[i].raw_data_size % pe.file_alignment == 0)",
    "for 2 i in (0..pe.number_of_sections - 1) : \
     (pe.sections[i].characteristics == 0x40000040)",
    r#"for any s in pe.sections : (s.name == ".text" and s.characteristics & pe.SECTION_CNT_CODE != 0)"#,
    "for all s in pe.sections : (s.virtual_address >= 0x1000)",
    r#"for 2 s in pe.sections : (s.name startswith ".r")"#,
    r#"pe.section_index(".text") == 0 and pe.section_index(".rsrc") == 2"#,
    "pe.section_index(pe.entry_point) == 0",
    "pe.rva_to_offset(pe.sections[1].virtual_address) == pe.sections[1].raw_data_offset",
    // 越界下标 = 确定的未定义（比较为假、defined 为假）
    "not defined pe.sections[3].name",
    "not defined pe.sections[pe.number_of_sections].name",
    "not defined pe.sections[0 - 1].name",
    "not defined pe.sections[2147483647].name",
    "not defined pe.sections[9223372036854775807].name",
    "not defined pe.sections[uint8(100000)].name",
    "for any i in (0..pe.number_of_sections) : (not defined pe.sections[i].name)",
    r#"not defined pe.section_index(".nope")"#,
    "not defined pe.rva_to_offset(0x7fffffff)",
    r#"not (pe.sections[0].name == ".data" or false)"#,
    // 官方把数组下标截成 C `int`：2^32 + k 就是第 k 项
    r#"pe.sections[4294967296].name == ".text""#,
    "pe.sections[4294967297].characteristics == pe.sections[1].characteristics",
    "pe.sections[4294967298].raw_data_size > 0 and pe.data_directories[4294967297].size == 60",
];

// probe: sample=rich
const SECTION_MISS: &[&str] = &[
    r#"pe.sections[3].name == ".text""#,
    r#"pe.sections[3].name != ".text""#,
    "pe.sections[pe.number_of_sections].virtual_address == 0",
    "pe.sections[0 - 1].virtual_address >= 0",
    // `not 未定义` 仍是未定义
    r#"not (pe.sections[3].name == ".text")"#,
    "for all i in (0..pe.number_of_sections) : (defined pe.sections[i].name)",
    r#"pe.sections[0].name == ".data""#,
    r#"for any s in pe.sections : (s.name == ".data")"#,
    r#"pe.section_index(".TEXT") == 0"#,
    // 截断后是负数（2^31）或越界（2^32 + 3）→ 未定义
    "pe.sections[2147483648].characteristics >= 0",
    "pe.sections[4294967299].characteristics >= 0",
];

#[test]
fn headers_and_dynamic_section_indexing() {
    assert_all(HEADER_HIT, Ternary::True);
    assert_all(SECTION_HIT, Ternary::True);
    assert_all(SECTION_MISS, Ternary::False);
}

// probe: sample=rich
const IMPORT_HIT: &[&str] = &[
    "pe.number_of_imports == 2 and pe.number_of_imported_functions == 3",
    r#"pe.import_details[0].library_name == "KERNEL32.dll""#,
    "pe.import_details[0].number_of_functions == 2",
    r#"pe.import_details[0].functions[0].name == "GetLastError""#,
    r#"pe.import_details[0].functions[1].name == "ExitProcess""#,
    "pe.import_details[0].functions[1].rva == 0x2070",
    "pe.import_details[1].functions[0].ordinal == 115",
    "not defined pe.import_details[0].functions[0].ordinal",
    "not defined pe.import_details[2].library_name",
    "not defined pe.import_details[0].functions[2].name",
    // 函数：库名 / 函数名不分大小写
    r#"pe.imports("KERNEL32.dll", "GetLastError")"#,
    r#"pe.imports("kernel32.DLL", "getlasterror")"#,
    r#"pe.imports("KERNEL32.dll") == 2"#,
    r#"pe.imports("WS2_32.dll") == 1"#,
    r#"pe.imports("WS2_32.dll", 115)"#,
    r#"pe.imports("nope.dll") == 0"#,
    r#"not pe.imports("KERNEL32.dll", "NoSuchFunction")"#,
    r#"not pe.imports("nope.dll", "GetLastError")"#,
    r#"pe.imports(pe.IMPORT_STANDARD, "KERNEL32.dll", "ExitProcess")"#,
    r#"pe.imports(pe.IMPORT_STANDARD, "KERNEL32.dll") == 2"#,
    r#"pe.imports(/kernel32/i, /^Get/) == 1"#,
    r#"pe.imports(/KERNEL32/, /Process$/) == 1"#,
    // import_rva：按导入表顺序第一个对上的；两个名字都不分大小写
    r#"pe.import_rva("KERNEL32.dll", "ExitProcess") == 0x2070"#,
    r#"pe.import_rva("kernel32.DLL", "exitprocess") == pe.import_details[0].functions[1].rva"#,
    r#"pe.import_rva("WS2_32.dll", 115) == pe.import_details[1].functions[0].rva"#,
    r#"not defined pe.import_rva("KERNEL32.dll", "NoSuchFunction")"#,
    r#"not defined pe.import_rva("nope.dll", "ExitProcess")"#,
    r#"not defined pe.import_rva("WS2_32.dll", 116)"#,
    r#"pe.pdb_path == "demo.pdb" and pe.pdb_path endswith ".pdb""#,
    "pe.data_directories[pe.IMAGE_DIRECTORY_ENTRY_DEBUG].size == 56",
    // 按序号导入的函数：官方用内置序号表合成名字（WS2_32 的 115 = WSAStartup）
    r#"pe.import_details[1].functions[0].name == "WSAStartup""#,
    r#"pe.imports("WS2_32.dll", "WSAStartup")"#,
    r#"pe.imports("ws2_32.DLL", "wsastartup") and not pe.imports("WS2_32.dll", "ord115")"#,
    r#"pe.import_rva("WS2_32.dll", "WSAStartup") == pe.import_rva("WS2_32.dll", 115)"#,
    r#"pe.imports(/ws2_32/i, /^WSA/) == 1"#,
    r#"for any imp in pe.import_details : (imp.library_name icontains "kernel32")"#,
    "for all imp in pe.import_details : (imp.number_of_functions > 0)",
    r#"for any imp in pe.import_details : (for any f in imp.functions : (f.name == "ExitProcess"))"#,
    "for all imp in pe.import_details : (for all f in imp.functions : (f.rva > 0x2000))",
    "for 1 imp in pe.import_details : (for any f in imp.functions : (defined f.ordinal))",
];

// probe: sample=rich
const EXPORT_HIT: &[&str] = &[
    "pe.number_of_exports == 3",
    r#"pe.export_details[0].name == "alpha" and pe.export_details[0].ordinal == 1"#,
    "pe.export_details[0].rva == 0x1000 and pe.export_details[0].offset == 0x400",
    "pe.export_details[1].ordinal == 2 and pe.export_details[1].offset == 0x408",
    "not defined pe.export_details[1].name",
    r#"pe.export_details[2].name == "beta""#,
    r#"pe.export_details[2].forward_name == "OTHER.target""#,
    // 转发导出没有文件偏移
    "not defined pe.export_details[2].offset",
    "not defined pe.export_details[0].forward_name",
    "not defined pe.export_details[3].ordinal",
    r#"pe.exports("alpha") and pe.exports("ALPHA") and pe.exports("beta")"#,
    "pe.exports(1) and pe.exports(2) and pe.exports(3)",
    r#"pe.exports(/^al/) and pe.exports(/ETA$/i)"#,
    r#"not pe.exports("gamma") and not pe.exports(0) and not pe.exports(4)"#,
    r#"pe.exports_index("alpha") == 0 and pe.exports_index("beta") == 2"#,
    "pe.exports_index(2) == 1",
    r#"pe.exports_index(/^b/) == 2"#,
    r#"not defined pe.exports_index("gamma")"#,
    "not defined pe.exports_index(0)",
    "for any e in pe.export_details : (defined e.forward_name)",
    "for 2 e in pe.export_details : (defined e.name)",
    "for all e in pe.export_details : (e.ordinal >= 1 and e.ordinal <= 3)",
    "for all i in (0..pe.number_of_exports - 1) : (pe.export_details[i].ordinal == i + 1)",
];

// probe: sample=rich
const RESOURCE_HIT: &[&str] = &[
    "pe.number_of_resources == 2",
    "pe.resources[0].type == pe.RESOURCE_TYPE_RCDATA and pe.resources[0].length == 16",
    "pe.resources[0].rva == 0x30B0 and pe.resources[0].offset == 0xAB0",
    r#"pe.resources[0].name_string == "C\x00F\x00G\x00""#,
    "not defined pe.resources[0].id",
    "pe.resources[1].type == pe.RESOURCE_TYPE_VERSION and pe.resources[1].id == 1",
    "pe.resources[1].language == 0x409 and pe.resources[1].offset == 0xAC0",
    "not defined pe.resources[1].name_string",
    "not defined pe.resources[2].type",
    "for any r in pe.resources : (r.type == pe.RESOURCE_TYPE_VERSION)",
    "for all r in pe.resources : (r.language == 1033 and r.offset + r.length <= filesize)",
    "for all i in (0..pe.number_of_resources - 1) : (pe.resources[i].rva >= 0x3000)",
    // 资源数据就在文件里：按偏移回读
    "uint32be(pe.resources[0].offset) == 0x636f6e66",
    "pe.number_of_version_infos == 3",
    r#"pe.version_info["CompanyName"] == "Trit Test Works""#,
    r#"pe.version_info["FileVersion"] == "1.2.3""#,
    r#"pe.version_info["OriginalFilename"] iequals "DEMO.DLL""#,
    r#"pe.version_info["CompanyName"] contains "Test" and pe.version_info["FileVersion"] matches /^1\.\d/"#,
    // 字典键区分大小写；没有的键 = 未定义
    r#"not defined pe.version_info["companyname"]"#,
    r#"not defined pe.version_info["Nope"]"#,
    r#"pe.version_info_list[0].key == "CompanyName""#,
    r#"pe.version_info_list[2].value == "demo.dll""#,
    "not defined pe.version_info_list[3].key",
    r#"for any k, v in pe.version_info : (k == "FileVersion" and v startswith "1.")"#,
    r#"for all k, v in pe.version_info : (pe.version_info[k] == v)"#,
    r#"for 3 k, v in pe.version_info : (v != "")"#,
    r#"for any v in pe.version_info_list : (v.key == "FileVersion" and v.value == "1.2.3")"#,
];

// probe: sample=rich
const TABLE_MISS: &[&str] = &[
    r#"pe.imports("KERNEL32.dll", "NoSuchFunction")"#,
    r#"pe.imports("KERNEL32.dll") == 3"#,
    r#"pe.import_details[2].library_name == "KERNEL32.dll""#,
    r#"for any imp in pe.import_details : (imp.library_name == "USER32.dll")"#,
    r#"pe.exports("gamma")"#,
    r#"pe.exports_index("gamma") == 0"#,
    r#"not (pe.exports_index("gamma") == 0)"#,
    "pe.export_details[2].offset == 0",
    "pe.resources[2].type == 0",
    "for any r in pe.resources : (r.type == pe.RESOURCE_TYPE_ICON)",
    r#"pe.version_info["Nope"] == """#,
    r#"pe.version_info["Nope"] contains "x""#,
    r#"not (pe.version_info["Nope"] contains "x")"#,
    r#"pe.version_info["companyname"] == "Trit Test Works""#,
];

#[test]
fn import_export_resource_and_version_tables() {
    assert_all(IMPORT_HIT, Ternary::True);
    assert_all(EXPORT_HIT, Ternary::True);
    assert_all(RESOURCE_HIT, Ternary::True);
    assert_all(TABLE_MISS, Ternary::False);
}

// 子集外的字段 / 函数：类型表里有（解析通过），求值记名弃权
// probe: sample=rich
const PE_UNKNOWN: &[&str] = &[
    "pe.number_of_signatures == 0",
    "pe.is_signed == 0",
    r#"pe.signatures[0].issuer contains "x""#,
    "pe.signatures[0].valid_on(1)",
    "pe.number_of_delayed_imports == 0",
    r#"pe.delayed_import_details[0].library_name == "a""#,
    "pe.checksum == pe.calculate_checksum()",
    r#"pe.imphash() == "x""#,
    "pe.locale(0x0409)",
    "pe.language(0x09)",
    "pe.rich_signature.toolid(1) > 0",
    r#"pe.delayed_import_rva("KERNEL32.dll", "ExitProcess") > 0"#,
    // 延迟导入表未建模：问到它就弃权
    r#"pe.imports(pe.IMPORT_DELAYED, "KERNEL32.dll") == 0"#,
    r#"pe.imports(pe.IMPORT_ANY, "nope.dll", "f")"#,
];

#[test]
fn fields_outside_the_subset_abstain_by_name() {
    assert_all(PE_UNKNOWN, Ternary::Unknown);
    let data = rich_pe();
    // 弃权不折成假：Kleene 短路照常
    // probe: sample=rich
    for (condition, expected) in [
        ("pe.is_signed == 0 or true", Ternary::True),
        ("pe.is_signed == 0 and false", Ternary::False),
        ("not (pe.is_signed == 0)", Ternary::Unknown),
        ("defined pe.is_signed", Ternary::Unknown),
        (
            "pe.is_signed == 0 or pe.number_of_sections == 3",
            Ternary::True,
        ),
        (
            "pe.is_signed == 0 and pe.number_of_sections == 4",
            Ternary::False,
        ),
        (
            "for all s in pe.sections : (pe.is_signed == 0)",
            Ternary::Unknown,
        ),
        ("for any s in pe.signatures : (true)", Ternary::Unknown),
        // 标准表里找到了就是找到了：不必再问延迟表
        (
            r#"pe.imports(pe.IMPORT_ANY, "KERNEL32.dll", "ExitProcess")"#,
            Ternary::True,
        ),
    ] {
        assert_eq!(eval_pe(condition, &data), expected, "{condition}");
    }
}

// probe: sample=rich
const PE_REJECT: &[&str] = &[
    "pe.nope == 1 + 0",
    "pe.sections[0].nope + 0 == 1",
    "pe.NOPE_CONST + 0 == 1",
    "pe.sections + 0 == 1",
    "pe.sections[0] + 0 == 1",
    "pe.sections.name + 0 == 1",
    r#"pe.sections["a"].virtual_address + 0 == 1"#,
    "pe.version_info[1] contains \"a\"",
    "pe.machine.x + 0 == 1",
    "pe.machine[0] + 0 == 1",
    "pe.machine() + 0 == 1",
    "pe.is_dll + 0 == 1",
    "pe.is_dll(1) + 0 == 1",
    "pe.imports() + 0 == 1",
    "pe.imports(1) + 0 == 1",
    r#"pe.imports("a", "b", "c") + 0 == 1"#,
    r#"pe.exports("a", "b") or 1 + 1 == 2"#,
    r#"pe.rva_to_offset("a") + 0 == 1"#,
    "pe.sections[0].name + 1 == 1",
    r#"pe.machine contains "a""#,
    "for any s in pe.machine : (true)",
    "for any s in pe : (true)",
    "for any s, t in pe.sections : (true)",
    "for any k in pe.version_info : (true)",
    "for any s in pe.sections : (s + 0 == 1)",
    "for any s in pe.sections : (s.nope + 0 == 1)",
    // 旧字段名（官方：invalid field name）
    "pe.sections[0].raw_size + 0 == 1",
];

#[test]
fn type_errors_against_the_module_schema_reject_the_whole_rule() {
    for condition in PE_REJECT {
        let reason = rejected(condition);
        assert!(!reason.is_empty(), "{condition}");
    }
    assert!(rejected("pe.nope == 1 + 0").contains("nope"));
}

#[test]
fn missing_pe_context_abstains_and_never_reads_as_false() {
    // 库直调不给 PE 上下文：pe.* 一律弃权（官方在非 PE 上给未定义——这里
    // 没有"它不是 PE"的证据，不替它下结论）
    for condition in [
        "pe.number_of_sections + 0 == 3",
        "pe.number_of_imports == 2",
        r#"pe.sections[0].name == ".text""#,
        "not defined pe.sections[99].name",
        "defined pe.sections[0].name",
        r#"pe.imports("KERNEL32.dll", "ExitProcess")"#,
        r#"not pe.imports("KERNEL32.dll", "ExitProcess")"#,
        r#"pe.version_info["CompanyName"] contains "Trit""#,
        "for any s in pe.sections : (true)",
        "for all s in pe.sections : (false)",
        "pe.is_pe",
        "not pe.is_pe",
    ] {
        assert_eq!(cond(condition), Ternary::Unknown, "{condition}");
    }
    // 模块常量不依赖样本
    assert_eq!(cond("pe.MACHINE_AMD64 == 0x8664"), Ternary::True);
    assert_eq!(cond("pe.IMPORT_ANY == -1 + 0"), Ternary::True);
    assert_eq!(
        cond("pe.number_of_sections + 0 == 3 or true"),
        Ternary::True
    );
}

#[test]
fn damaged_tables_never_panic() {
    // 把夹具逐字节砸坏 / 截短：结论可以变、可以弃权，但不能 panic 不能挂起
    let data = rich_pe();
    let conditions = [
        "pe.number_of_imports + pe.number_of_exports + pe.number_of_resources >= 0",
        "for all imp in pe.import_details : (for all f in imp.functions : (f.rva >= 0))",
        r#"for any e in pe.export_details : (e.name == "alpha" or defined e.forward_name)"#,
        r#"for all k, v in pe.version_info : (k != "" and v != "")"#,
        "for all r in pe.resources : (r.offset + r.length >= 0)",
        r#"pe.imports(/./, /./) >= 0 or pe.exports(/./) or pe.section_index(".rsrc") >= 0"#,
        "for all i in (0..20) : (pe.sections[i].virtual_address >= 0 or true)",
    ];
    let rules: Vec<_> = conditions
        .iter()
        .map(|condition| yara_expr_support::parse("", condition).expect("可解析"))
        .collect();
    let scan = |bytes: &[u8]| {
        let info = trit_re::pe::parse(bytes).ok();
        trit_re::sigscan::SigScan::new(&rules).scan_with_context(bytes, false, None, info.as_ref());
    };
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..400 {
        let mut mutated = data.clone();
        for _ in 0..(1 + next() % 4) {
            let at = (next() % mutated.len() as u64) as usize;
            mutated[at] = match next() % 3 {
                0 => 0xFF,
                1 => 0,
                _ => next() as u8,
            };
        }
        scan(&mutated);
    }
    for cut in (0..data.len()).step_by(37) {
        scan(&data[..cut]);
    }
}
