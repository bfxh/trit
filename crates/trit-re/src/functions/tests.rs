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
    let report = attribute(&xref, &[], true, vec![]);
    assert_eq!(report.unattributed_calls, 1);
    assert_eq!(report.unattributed_jumps, 1);
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("call 1 / jmp 1"))
    );
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
