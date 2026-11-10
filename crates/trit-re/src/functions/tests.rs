//! `functions` 的单元测试（从 mod.rs 拆出，控制单文件行数预算）。

use super::{FuncSource, Function, FunctionCalls, FunctionReport, locate};
use trit_core::value::Ternary;

fn cooccurrence_case() -> FunctionReport {
    FunctionReport {
        functions: vec![FunctionCalls {
            function: function(0x1000, 0x1100),
            imports: vec!["a!x".into(), "a!y".into()],
            buckets: vec!["a", "b"],
        }],
        total_functions: 1,
        exact: true,
        attributed_sites: 2,
        unattributed_sites: 0,
        unattributed_calls: 0,
        unattributed_jumps: 0,
        unattributed_buckets: vec![],
        ruled_sites: vec![],
        ruling_clip: 0,
        ruling_miss: 0,
        ruling_nonexec: 0,
        ruling_thunk_gap: 0,
        notes: vec![],
    }
}

fn function(start: u64, end: u64) -> Function {
    Function {
        start,
        end,
        name: None,
        source: FuncSource::Pdata,
    }
}

#[test]
fn exact_coverage_needs_ninety_percent() {
    use super::coverage_is_complete;
    assert!(coverage_is_complete(900, 1000));
    assert!(coverage_is_complete(1000, 1000));
    assert!(!coverage_is_complete(899, 1000));
    assert!(!coverage_is_complete(0, 0));
    assert!(!coverage_is_complete(10, 0));
}

#[test]
fn eh_frame_boundaries_enable_attribution() {
    use super::build_elf;
    use crate::elf::{ElfInfo, ElfSection};
    use crate::xref::{CallSite, ImportXref, SiteKind as Kind, XrefReport};
    let mut bytes = vec![0u8; 0x140];
    let put32 = |bytes: &mut [u8], off: usize, value: u32| {
        bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
    };
    // CIE（同上）与 FDE：.eh_frame 的地址是 0x2000，pcrel 目标 0x1100。
    put32(&mut bytes, 0, 13);
    bytes[8] = 1;
    bytes[9] = b'z';
    bytes[10] = b'R';
    bytes[12] = 1;
    bytes[13] = 0x78;
    bytes[14] = 30;
    bytes[15] = 1;
    bytes[16] = 0x1b;
    put32(&mut bytes, 17, 12);
    put32(&mut bytes, 21, 21);
    put32(&mut bytes, 25, 0x1100u32.wrapping_sub(0x2019));
    put32(&mut bytes, 29, 0x40);
    let info = ElfInfo {
        class: 2,
        etype: 2,
        machine: 62,
        entry: 0x1100,
        phdrs: Vec::new(),
        sections: vec![
            ElfSection {
                name: ".text".into(),
                sh_type: 1,
                flags: 6,
                addr: 0x1000,
                offset: 0x40,
                size: 0x100,
                link: 0,
                entsize: 0,
                entropy: None,
            },
            ElfSection {
                name: ".eh_frame".into(),
                sh_type: 1,
                flags: 2,
                addr: 0x2000,
                offset: 0,
                size: 0x40,
                link: 0,
                entsize: 0,
                entropy: None,
            },
        ],
        imports: None,
        comment: None,
        build_id: None,
        interp: None,
        overlay: None,
        has_section_headers: true,
    };
    let xref = XrefReport {
        arch: "arm64".into(),
        scanned_bytes: 0,
        complete: true,
        imports: vec![ImportXref {
            import: "socket".into(),
            slot: 1,
            sites: vec![CallSite {
                addr: 0x1108,
                file_offset: 0x48,
                kind: Kind::Call,
            }],
            status: Ternary::True,
            note: String::new(),
        }],
        notes: vec![],
    };
    let report = build_elf(&bytes, &info, &xref);
    assert!(report.exact);
    assert_eq!(report.total_functions, 1);
    assert_eq!(report.functions.len(), 1);
    assert_eq!(
        report.functions[0].function.source,
        super::FuncSource::EhFrame
    );
    assert_eq!(report.attributed_sites, 1);
    assert!(report.notes.iter().any(|note| note.contains(".eh_frame")));
}

#[test]
fn debug_frame_boundaries_enable_attribution() {
    use super::build_elf;
    use crate::elf::{ElfInfo, ElfSection};
    use crate::xref::{CallSite, ImportXref, SiteKind as Kind, XrefReport};
    let mut bytes = vec![0u8; 0x140];
    let put32 = |bytes: &mut [u8], off: usize, value: u32| {
        bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
    };
    let put64 = |bytes: &mut [u8], off: usize, value: u64| {
        bytes[off..off + 8].copy_from_slice(&value.to_le_bytes());
    };
    // CIE（v4，address_size=8）与 FDE（绝对地址 0x1100，范围 0x40）。
    put32(&mut bytes, 0, 11);
    put32(&mut bytes, 4, 0xffff_ffff);
    bytes[8] = 4;
    bytes[9] = 0;
    bytes[10] = 8;
    bytes[11] = 0;
    bytes[12] = 1;
    bytes[13] = 0x78;
    bytes[14] = 30;
    put32(&mut bytes, 15, 20);
    put32(&mut bytes, 19, 0);
    put64(&mut bytes, 23, 0x1100);
    put64(&mut bytes, 31, 0x40);
    let info = ElfInfo {
        class: 2,
        etype: 2,
        machine: 62,
        entry: 0x1100,
        phdrs: Vec::new(),
        sections: vec![
            ElfSection {
                name: ".text".into(),
                sh_type: 1,
                flags: 6,
                addr: 0x1000,
                offset: 0x40,
                size: 0x100,
                link: 0,
                entsize: 0,
                entropy: None,
            },
            ElfSection {
                name: ".debug_frame".into(),
                sh_type: 1,
                flags: 0,
                addr: 0,
                offset: 0,
                size: 0x40,
                link: 0,
                entsize: 0,
                entropy: None,
            },
        ],
        imports: None,
        comment: None,
        build_id: None,
        interp: None,
        overlay: None,
        has_section_headers: true,
    };
    let xref = XrefReport {
        arch: "x86-64".into(),
        scanned_bytes: 0,
        complete: true,
        imports: vec![ImportXref {
            import: "socket".into(),
            slot: 1,
            sites: vec![CallSite {
                addr: 0x1108,
                file_offset: 0x48,
                kind: Kind::Call,
            }],
            status: Ternary::True,
            note: String::new(),
        }],
        notes: vec![],
    };
    let report = build_elf(&bytes, &info, &xref);
    assert!(report.exact);
    assert_eq!(report.total_functions, 1);
    assert_eq!(
        report.functions[0].function.source,
        super::FuncSource::DebugFrame
    );
    assert_eq!(report.attributed_sites, 1);
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains(".debug_frame"))
    );
}

#[test]
fn frame_source_prefers_the_one_covering_call_sites() {
    use super::{FuncSource, choose_frame_source};
    use crate::elf::FunctionSymbol;
    use crate::xref::{CallSite, ImportXref, SiteKind as Kind, XrefReport};
    let eh = vec![FunctionSymbol {
        start: 0x2000,
        size: 0x100,
        name: String::new(),
    }];
    let debug = vec![FunctionSymbol {
        start: 0x1000,
        size: 0x100,
        name: String::new(),
    }];
    let xref = |addr: u64| XrefReport {
        arch: "x86-64".into(),
        scanned_bytes: 0,
        complete: true,
        imports: vec![ImportXref {
            import: "socket".into(),
            slot: 1,
            sites: vec![CallSite {
                addr,
                file_offset: 0,
                kind: Kind::Call,
            }],
            status: Ternary::True,
            note: String::new(),
        }],
        notes: vec![],
    };
    let (source, _, other) = choose_frame_source(&eh, &debug, &xref(0x1010)).unwrap();
    assert_eq!(source, FuncSource::DebugFrame);
    assert_eq!(other, Some(0));
    let (source, _, other) = choose_frame_source(&eh, &debug, &xref(0x2010)).unwrap();
    assert_eq!(source, FuncSource::EhFrame);
    assert_eq!(other, Some(0));
}

#[test]
fn unattributed_sites_are_split_by_kind() {
    use super::attribute;
    use crate::xref::{CallSite, ImportXref, SiteKind as Kind, XrefReport};
    let xref = XrefReport {
        arch: "test".into(),
        scanned_bytes: 0,
        complete: true,
        imports: vec![ImportXref {
            import: "a!VirtualAllocEx".into(),
            slot: 1,
            sites: vec![
                CallSite {
                    addr: 0x100,
                    file_offset: 0,
                    kind: Kind::Call,
                },
                CallSite {
                    addr: 0x200,
                    file_offset: 0,
                    kind: Kind::Jump,
                },
            ],
            status: Ternary::True,
            note: String::new(),
        }],
        notes: vec![],
    };
    let report = attribute(&xref, &[], &[], true, vec![]);
    assert_eq!(report.unattributed_calls, 1);
    assert_eq!(report.unattributed_jumps, 1);
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("call 1 / jmp 1"))
    );
    // 逐条裁定：没有 exec 段时两个调用点都是非执行节（误报候选）。
    assert_eq!(report.ruling_nonexec, 2);
    assert_eq!(report.ruling_clip, 0);
    assert_eq!(report.ruling_miss, 0);
    assert_eq!(report.ruled_sites.len(), 2);
    assert!(
        report
            .ruled_sites
            .iter()
            .all(|site| site.ruling.as_str() == "non-exec")
    );
}

#[test]
fn e9_to_jmp_thunk_in_gap_is_ruled_thunk_gap() {
    // 实测形态（trit-re.exe 全部 8 个残余）：`E9 rel32 → FF 25`（两级跳板
    // 链）落在函数间隙——增量链接死跳板。裁定如实单列，不再伪装成边界漏检。
    use super::attribute;
    use crate::xref::{CallSite, ImportXref, SiteKind as Kind, XrefReport};
    let mut code = vec![0xCCu8; 64];
    // 0x110: E9 +0 → 0x115；0x115: FF 25（jmp rip-rel 跳板）
    code[0x10..0x15].copy_from_slice(&[0xE9, 0x00, 0x00, 0x00, 0x00]);
    code[0x15..0x17].copy_from_slice(&[0xFF, 0x25]);
    let xref = XrefReport {
        arch: "test".into(),
        scanned_bytes: 0,
        complete: true,
        imports: vec![ImportXref {
            import: "a!LoadLibraryW".into(),
            slot: 1,
            sites: vec![CallSite {
                addr: 0x110,
                file_offset: 0,
                kind: Kind::Jump,
            }],
            status: Ternary::True,
            note: String::new(),
        }],
        notes: vec![],
    };
    // 无精确函数区间：站点在 exec 内但归不到函数
    let report = attribute(&xref, &[], &[(0x100, &code)], true, vec![]);
    assert_eq!(report.ruling_thunk_gap, 1);
    assert_eq!(report.ruling_miss, 0, "跳板链不再伪装成边界漏检");
    assert_eq!(report.ruled_sites[0].ruling.as_str(), "thunk-gap");
    // E9 指向普通代码（非 FF 25 跳板）→ 不是间隙跳板，按距离裁定
    let mut code2 = vec![0xCCu8; 64];
    code2[0x10..0x15].copy_from_slice(&[0xE9, 0x10, 0x00, 0x00, 0x00]); // → 0x125（普通地址）
    let xref2 = XrefReport {
        arch: "test".into(),
        scanned_bytes: 0,
        complete: true,
        imports: vec![ImportXref {
            import: "a!LoadLibraryW".into(),
            slot: 1,
            sites: vec![CallSite {
                addr: 0x110,
                file_offset: 0,
                kind: Kind::Jump,
            }],
            status: Ternary::True,
            note: String::new(),
        }],
        notes: vec![],
    };
    let report = attribute(&xref2, &[], &[(0x100, &code2)], true, vec![]);
    assert_eq!(report.ruling_thunk_gap, 0);
    assert_eq!(report.ruling_miss, 1, "非跳板链保持原裁定");
}

#[test]
fn locate_respects_half_open_ranges() {
    let ranges = vec![function(0x1000, 0x1100), function(0x1100, 0x1200)];
    assert_eq!(locate(&ranges, 0x1000), Some(0));
    assert_eq!(locate(&ranges, 0x10ff), Some(0));
    assert_eq!(locate(&ranges, 0x1100), Some(1));
    assert_eq!(locate(&ranges, 0x1200), None);
    assert_eq!(locate(&ranges, 0x0fff), None);
}

#[test]
fn cooccurrence_requires_exact_and_fully_attributed() {
    let report = FunctionReport {
        functions: vec![],
        total_functions: 1,
        exact: true,
        attributed_sites: 0,
        unattributed_sites: 0,
        unattributed_calls: 0,
        unattributed_jumps: 0,
        unattributed_buckets: vec![],
        ruled_sites: vec![],
        ruling_clip: 0,
        ruling_miss: 0,
        ruling_nonexec: 0,
        ruling_thunk_gap: 0,
        notes: vec![],
    };
    assert_eq!(report.cooccurrence(&["a", "b"], true), Ternary::False);
    assert_eq!(report.cooccurrence(&["a", "b"], false), Ternary::Unknown);
    let loose = FunctionReport {
        exact: false,
        ..report.clone()
    };
    assert_eq!(loose.cooccurrence(&["a", "b"], true), Ternary::Unknown);
    let relevant_gap = FunctionReport {
        unattributed_sites: 1,
        unattributed_buckets: vec!["a"],
        ..report.clone()
    };
    assert_eq!(
        relevant_gap.cooccurrence(&["a", "b"], true),
        Ternary::Unknown
    );
    // 与本组无关的未归属调用点不影响结论。
    let irrelevant_gap = FunctionReport {
        unattributed_sites: 1,
        unattributed_calls: 1,
        unattributed_buckets: vec!["other"],
        ..report.clone()
    };
    assert_eq!(
        irrelevant_gap.cooccurrence(&["a", "b"], true),
        Ternary::False
    );
    let hit = cooccurrence_case();
    assert_eq!(hit.cooccurrence(&["a", "b"], true), Ternary::True);
    // 正证据优先：即使有未归属调用点，找到共现仍然给 True。
    let hit_with_gap = FunctionReport {
        unattributed_sites: 3,
        unattributed_calls: 3,
        unattributed_buckets: vec!["a"],
        ..cooccurrence_case()
    };
    assert_eq!(hit_with_gap.cooccurrence(&["a", "b"], true), Ternary::True);
}

#[test]
fn unattributed_sites_get_per_site_rulings() {
    use super::{BOUNDARY_CLIP_BYTES, attribute};
    use crate::xref::{CallSite, ImportXref, SiteKind as Kind, XrefReport};

    // 一个函数 [0x1000, 0x1100)；exec 段 [0x0, 0x2000)。
    // 三个未归属调用点分别命中三类裁定：
    // - 0x1103：距函数末尾 4 字节 ⇒ 疑边界裁剪（对齐填充/尾部代码）；
    // - 0x1800：exec 内、距边界 0x700 ⇒ 疑边界漏检；
    // - 0x3000：exec 段之外 ⇒ 字节模式误报候选。
    let xref = |sites: &[(u64, Kind)]| XrefReport {
        arch: "test".into(),
        scanned_bytes: 0,
        complete: true,
        imports: sites
            .iter()
            .map(|(addr, kind)| ImportXref {
                import: "a!VirtualAllocEx".into(),
                slot: 1,
                sites: vec![CallSite {
                    addr: *addr,
                    file_offset: 0,
                    kind: *kind,
                }],
                status: Ternary::Unknown,
                note: String::new(),
            })
            .collect(),
        notes: vec![],
    };
    let ranges = vec![function(0x1000, 0x1100)];
    let exec_data = vec![0u8; 0x2000];
    let exec = vec![(0x0u64, &exec_data[..])];

    let report = attribute(
        &xref(&[
            (0x1103, Kind::Call),
            (0x1800, Kind::Call),
            (0x3000, Kind::Jump),
        ]),
        &ranges,
        &exec,
        true,
        vec![],
    );
    assert_eq!(report.unattributed_sites, 3);
    assert_eq!(report.ruling_clip, 1);
    assert_eq!(report.ruling_miss, 1);
    assert_eq!(report.ruling_nonexec, 1);
    assert_eq!(report.ruled_sites.len(), 3);
    let ruling_of = |addr: u64| {
        report
            .ruled_sites
            .iter()
            .find(|site| site.addr == addr)
            .expect("三个地址都要有裁定")
    };
    assert_eq!(ruling_of(0x1103).ruling.as_str(), "boundary-clip");
    assert_eq!(ruling_of(0x1103).distance, 4);
    assert_eq!(ruling_of(0x1800).ruling.as_str(), "boundary-miss");
    assert!(ruling_of(0x1800).distance > BOUNDARY_CLIP_BYTES);
    assert_eq!(ruling_of(0x3000).ruling.as_str(), "non-exec");
    // 说明里带三类计数的汇总。
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("逐条裁定") && note.contains("非执行节（误报候选）1"))
    );
}

#[test]
fn ruling_detail_is_capped_but_counts_are_complete() {
    use super::{MAX_RULED_SITES, attribute};
    use crate::xref::{CallSite, ImportXref, SiteKind as Kind, XrefReport};

    // 100 个未归属调用点（全在 exec 外）：明细截断到 64，三类计数仍为 100。
    let sites: Vec<(u64, Kind)> = (0..100)
        .map(|i| (0x3000 + i as u64 * 0x10, Kind::Call))
        .collect();
    let xref = XrefReport {
        arch: "test".into(),
        scanned_bytes: 0,
        complete: true,
        imports: sites
            .iter()
            .map(|(addr, kind)| ImportXref {
                import: "a!x".into(),
                slot: 1,
                sites: vec![CallSite {
                    addr: *addr,
                    file_offset: 0,
                    kind: *kind,
                }],
                status: Ternary::Unknown,
                note: String::new(),
            })
            .collect(),
        notes: vec![],
    };
    let report = attribute(&xref, &[], &[], true, vec![]);
    assert_eq!(report.unattributed_sites, 100);
    assert_eq!(report.ruled_sites.len(), MAX_RULED_SITES);
    assert_eq!(report.ruling_nonexec, 100);
}

/// v4：数据节指针表收割的过滤规则——连续 ≥2 的可执行域指针全收，
/// 孤立指针仅 16 对齐收，NULL/非可执行域值归零连击。
#[test]
fn pointer_table_harvest_filters() {
    use crate::pe::{PeInfo, Section};
    let plus = true;
    let image_base = 0x140000000u64;
    // 数据节原始内容：VA 串 = [叶子A, 叶子B, 叶子C, 0(空槽), 整数巧合(指向 exec 的孤立非对齐)]
    let mut table = Vec::new();
    for rva in [0x1010u64, 0x1030u64, 0x1050u64, 0u64] {
        table.extend((image_base + rva).to_le_bytes());
    }
    table.extend((image_base + 0x1099u64).to_le_bytes()); // 孤立、非 16 对齐
    let mut data_bytes = table;
    data_bytes.resize(0x100, 0); // 后续全 0（NULL 槽位，不收）
    let info = PeInfo {
        plus,
        machine: 0x8664,
        timestamp: 0,
        subsystem: 3,
        entry_rva: 0x1000,
        image_base,
        sections: vec![Section {
            name: ".rdata".into(),
            virtual_size: data_bytes.len() as u32,
            virtual_address: 0x2000,
            raw_size: data_bytes.len() as u32,
            raw_ptr: 0,
            characteristics: 0x4000_0040, // initialized data，不可执行
            entropy: None,
        }],
        imports: None,
        exports: Vec::new(),
        debug_pdb: None,
        has_security_dir: false,
        has_signed_data: false,
        rich_header: false,
        runtime_functions: None,
        coff_functions: None,
        clr_dir_present: false,
        clr: None,
        tls_dir_present: false,
        tls_callbacks: None,
        overlay: None,
        headers_size: 0x400,
    };
    let exec_bounds = [(0x1000u64, 0x1100u64)];
    let seeds = super::pe_pointer_seeds(&data_bytes, &info, &exec_bounds);
    for want in [0x1010u64, 0x1030u64, 0x1050u64] {
        assert!(seeds.contains(&want), "连击表里的函数指针全收：{seeds:?}");
    }
    assert!(
        !seeds.contains(&0x1099),
        "孤立非对齐指针 = 整数巧合，丢（宁可丢也不给假起点）"
    );
    assert!(!seeds.contains(&0), "NULL 槽位不收");
}
