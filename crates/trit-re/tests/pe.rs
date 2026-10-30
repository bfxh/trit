//! 合成 PE32+ 的端到端解析与判定。

mod support;

use trit_belnap::B4;
use trit_core::value::Ternary;
use trit_re::{analyze, pe};

#[test]
fn parses_headers_imports_exports_debug_and_overlay() {
    let bytes = support::pe_sample();
    let info = pe::parse(&bytes).expect("synthetic PE must parse");
    assert!(info.plus);
    assert_eq!(info.arch(), "x86-64");
    assert_eq!(info.format(), "pe32+");
    assert_eq!(info.sections.len(), 2);
    assert_eq!(info.sections[0].name, ".text");
    assert!(info.sections[0].executable());

    let dlls = info.imports.as_ref().expect("imports parsed");
    assert_eq!(dlls.len(), 1);
    assert_eq!(dlls[0].name, "KERNEL32.dll");
    assert_eq!(dlls[0].functions.len(), 1);
    assert_eq!(dlls[0].functions[0].name, "LoadLibraryA");
    assert_eq!(dlls[0].functions[0].slot_rva, Some(0x2040));

    assert_eq!(info.exports, vec!["DemoExport"]);
    assert_eq!(info.debug_pdb.as_deref(), Some("D:\\build\\demo.pdb"));
    assert!(info.has_security_dir);
    assert!(info.has_signed_data);
    assert!(info.rich_header);
    assert_eq!(info.entry_in_executable(), Some(true));

    let overlay = info.overlay.expect("overlay must be detected");
    assert_eq!(overlay.size, 8192);
    assert!(overlay.entropy > 7.5, "entropy={}", overlay.entropy);
}

#[test]
fn triage_flags_packed_structure_and_signature_conflict() {
    let triage = analyze(&support::pe_sample());
    assert_eq!(triage.format, "pe32+");
    assert_eq!(triage.facts.get("imports.loader"), Some(Ternary::True));
    assert_eq!(triage.facts.get("imports.network"), Some(Ternary::False));
    assert_eq!(
        triage.facts.get("overlay.high_entropy"),
        Some(Ternary::True)
    );
    assert_eq!(
        triage.facts.get("authenticode.present"),
        Some(Ternary::True)
    );

    let packed = triage.judgments.iter().find(|j| j.id == "packed").unwrap();
    assert_eq!(packed.value, Ternary::True);
    // 结构像壳 + 有签名结构 → 四值的 B（两种解释都有证据）。
    assert_eq!(packed.four, B4::B);

    let review = triage
        .judgments
        .iter()
        .find(|j| j.id == "needs_review")
        .unwrap();
    assert_eq!(review.value, Ternary::True);
}

#[test]
fn xref_finds_the_iat_call_site() {
    let triage = analyze(&support::pe_sample());
    let report = triage.xref.as_ref().expect("pe sample has xref");
    assert!(report.complete);
    let row = report
        .imports
        .iter()
        .find(|row| row.import == "KERNEL32.dll!LoadLibraryA")
        .expect("import row");
    assert_eq!(row.status, Ternary::True);
    assert_eq!(row.sites.len(), 1);
    assert_eq!(row.sites[0].addr, 0x1010);
    assert_eq!(row.sites[0].kind, trit_re::SiteKind::Call);
    // 事实与判定都要能吃到交叉引用结果。
    assert_eq!(triage.facts.get("xref.scan_complete"), Some(Ternary::True));
    let gaps = triage
        .judgments
        .iter()
        .find(|j| j.id == "xref_gaps")
        .unwrap();
    assert_eq!(gaps.value, Ternary::False);
}

#[test]
fn every_truncation_is_an_error_or_a_parse_never_a_panic() {
    let bytes = support::pe_sample();
    for cut in [
        0, 1, 0x3f, 0x40, 0x80, 0x98, 0x188, 0x1d8, 0x600, 0x700, 0xa00,
    ] {
        let slice = &bytes[..cut.min(bytes.len())];
        let _ = pe::parse(slice);
        let _ = analyze(slice);
    }
}
