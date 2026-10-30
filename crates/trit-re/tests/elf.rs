//! 合成 ELF64 的端到端解析与判定。

mod support;

use trit_core::value::Ternary;
use trit_re::{analyze, elf};

#[test]
fn parses_sections_dynamic_imports_and_notes() {
    let bytes = support::elf_sample(false, false);
    let info = elf::parse(&bytes).expect("synthetic ELF must parse");
    assert_eq!(info.format(), "elf64");
    assert_eq!(info.arch(), "x86-64");
    assert_eq!(info.sections.len(), 9);
    assert_eq!(
        info.imports.as_deref(),
        Some(["socket".to_string()].as_slice())
    );
    assert!(info.comment.as_deref().unwrap().contains("GCC"));
    assert_eq!(info.build_id.as_deref(), Some("deadbeef01020304"));
    assert_eq!(info.entry_in_executable(), Some(true));
    assert_eq!(info.overlay.as_ref().unwrap().size, 512);
}

#[test]
fn xref_follows_plt_to_the_caller() {
    let triage = analyze(&support::elf_sample(false, false));
    let report = triage.xref.as_ref().expect("elf sample has xref");
    assert!(report.complete);
    let row = report
        .imports
        .iter()
        .find(|row| row.import == "socket")
        .expect("socket row");
    assert_eq!(row.status, Ternary::True);
    assert_eq!(row.sites.len(), 1);
    assert_eq!(row.sites[0].addr, 0x400110);
    assert_eq!(row.sites[0].kind, trit_re::SiteKind::Call);
    let gaps = triage
        .judgments
        .iter()
        .find(|j| j.id == "xref_gaps")
        .unwrap();
    assert_eq!(gaps.value, Ternary::False);
    // 合成 ELF 没有 STT_FUNC 符号 ⇒ 函数边界是启发式，共现必须保持未知。
    let functions = triage.functions.as_ref().expect("function report");
    assert!(!functions.exact);
    assert_eq!(
        triage.facts.get("fn.stager_cooccur"),
        Some(Ternary::Unknown)
    );
}

#[test]
fn plain_binary_is_not_packed() {
    let triage = analyze(&support::elf_sample(false, false));
    assert_eq!(triage.facts.get("imports.network"), Some(Ternary::True));
    assert_eq!(
        triage.facts.get("authenticode.present"),
        Some(Ternary::False)
    );
    assert_eq!(
        triage.facts.get("overlay.high_entropy"),
        Some(Ternary::False)
    );
    let packed = triage.judgments.iter().find(|j| j.id == "packed").unwrap();
    assert_eq!(packed.value, Ternary::False);
    let review = triage
        .judgments
        .iter()
        .find(|j| j.id == "needs_review")
        .unwrap();
    assert_eq!(review.value, Ternary::False);
}

#[test]
fn high_entropy_overlay_is_packed() {
    let triage = analyze(&support::elf_sample(true, false));
    assert_eq!(
        triage.facts.get("overlay.high_entropy"),
        Some(Ternary::True)
    );
    let packed = triage.judgments.iter().find(|j| j.id == "packed").unwrap();
    assert_eq!(packed.value, Ternary::True);
    // ELF 无 Authenticode 概念，良性解释为假 → 不是 B。
    assert_eq!(packed.four.as_char(), 'T');
}

#[test]
fn stripped_section_table_turns_import_evidence_unknown() {
    let bytes = support::elf_sample(false, true);
    let info = elf::parse(&bytes).expect("stripped ELF still parses headers");
    assert!(!info.has_section_headers);
    assert!(info.imports.is_none());

    let triage = analyze(&bytes);
    assert_eq!(triage.facts.get("imports.loader"), Some(Ternary::Unknown));
    // 无节表 ⇒ 重定位也读不到，交叉引用必须保持未知而不是"没有调用点"。
    assert_eq!(
        triage.facts.get("xref.has_unreferenced"),
        Some(Ternary::Unknown)
    );
    let injection = triage
        .judgments
        .iter()
        .find(|j| j.id == "injection")
        .unwrap();
    assert_eq!(injection.value, Ternary::Unknown);
}
