//! ARM64（AArch64）的交叉引用与函数边界端到端用例。

mod support;

use trit_core::value::Ternary;
use trit_re::{SiteKind, analyze};

const LOAD_LIBRARY: &str = "KERNEL32.dll!LoadLibraryA";

fn row<'a>(triage: &'a trit_re::Triage, import: &str) -> &'a trit_re::ImportXref {
    triage
        .xref
        .as_ref()
        .expect("xref report")
        .imports
        .iter()
        .find(|row| row.import == import)
        .expect("import row")
}

#[test]
fn pe_arm64_direct_blr_is_attributed_to_the_function() {
    let triage = analyze(&support::pe_arm64_sample(false));
    assert_eq!(triage.format, "pe32+");
    assert_eq!(triage.arch, "arm64");

    let report = triage.xref.as_ref().expect("xref");
    assert!(report.complete);
    let load = row(&triage, LOAD_LIBRARY);
    assert_eq!(load.status, Ternary::True);
    assert_eq!(load.sites.len(), 1);
    assert_eq!(load.sites[0].addr, 0x1008);
    assert_eq!(load.sites[0].kind, SiteKind::Call);

    // ARM64 .pdata 是 8 字节条目：结束地址由下一条推，这里用可执行节末尾兜底。
    let functions = triage.functions.as_ref().expect("functions");
    assert!(functions.exact);
    assert_eq!(functions.total_functions, 1);
    assert_eq!(functions.attributed_sites, 1);
    assert_eq!(functions.unattributed_sites, 0);
    assert_eq!(triage.facts.get("fn.exact_boundaries"), Some(Ternary::True));
}

#[test]
fn pe_arm64_import_thunk_expands_to_the_caller() {
    let triage = analyze(&support::pe_arm64_sample(true));
    let load = row(&triage, LOAD_LIBRARY);
    assert_eq!(load.status, Ternary::True);
    assert_eq!(load.sites.len(), 1);
    // 跳板本身在 0x1020，真正的调用方在 0x1000。
    assert_eq!(load.sites[0].addr, 0x1000);
    assert_eq!(load.sites[0].kind, SiteKind::Call);
    let functions = triage.functions.as_ref().expect("functions");
    assert_eq!(functions.attributed_sites, 1);
    assert_eq!(functions.unattributed_sites, 0);
}

#[test]
fn elf_arm64_plt_call_is_found() {
    let triage = analyze(&support::elf_arm64_sample());
    assert_eq!(triage.format, "elf64");
    assert_eq!(triage.arch, "arm64");

    let report = triage.xref.as_ref().expect("xref");
    assert!(report.complete);
    let socket = row(&triage, "socket");
    assert_eq!(socket.status, Ternary::True);
    assert_eq!(socket.sites.len(), 1);
    assert_eq!(socket.sites[0].addr, 0x40_0110);
    assert_eq!(socket.sites[0].kind, SiteKind::Call);

    // 合成 ELF 没有 STT_FUNC ⇒ 函数边界是启发式，共现必须保持未知。
    let functions = triage.functions.as_ref().expect("functions");
    assert!(!functions.exact);
    assert_eq!(
        triage.facts.get("fn.stager_cooccur"),
        Some(Ternary::Unknown)
    );
}
