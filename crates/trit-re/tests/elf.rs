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
fn elf32_pic_stub_is_resolved_through_get_pc_thunk() {
    let triage = analyze(&support::elf32_sample(true));
    let report = triage.xref.as_ref().expect("elf32 sample has xref");
    assert!(report.complete);
    let socket = report
        .imports
        .iter()
        .find(|row| row.import == "socket")
        .expect("socket row");
    assert_eq!(socket.status, Ternary::True);
    // E8 → PLT 桩（call）、call *disp(%ebx)、jmp *abs32。
    assert_eq!(socket.sites.len(), 3);
    assert_eq!(
        socket
            .sites
            .iter()
            .filter(|site| site.kind == trit_re::SiteKind::Call)
            .count(),
        2
    );
    // printf 只有 GLOB_DAT 槽位（.rel.dyn），但确有 PLT 桩与调用：必须判 True。
    let printf = report
        .imports
        .iter()
        .find(|row| row.import == "printf")
        .expect("printf row");
    assert_eq!(printf.status, Ternary::True);
    assert_eq!(printf.sites.len(), 1);
    assert_eq!(printf.sites[0].kind, trit_re::SiteKind::Call);
    // getenv 只被 `push $getenv@plt` 取过地址：不能判"没有引用"。
    let getenv = report
        .imports
        .iter()
        .find(|row| row.import == "getenv")
        .expect("getenv row");
    assert_eq!(getenv.status, Ternary::Unknown);
    assert!(getenv.sites.is_empty());
    // memcmp 只有一次 `mov ecx,[eax+disp32]`：基址在 EAX（get_pc_thunk.ax），同上。
    let memcmp = report
        .imports
        .iter()
        .find(|row| row.import == "memcmp")
        .expect("memcmp row");
    assert_eq!(memcmp.status, Ternary::Unknown);
    assert!(memcmp.sites.is_empty());
}

#[test]
fn elf32_missing_got_base_marks_scan_incomplete() {
    let triage = analyze(&support::elf32_sample(false));
    let report = triage.xref.as_ref().expect("elf32 sample has xref");
    // `(%ebx)` 引用解析不了 ⇒ 不能声称扫全了。
    assert!(!report.complete);
    let printf = report
        .imports
        .iter()
        .find(|row| row.import == "printf")
        .expect("printf row");
    assert_eq!(printf.status, Ternary::Unknown);
    // 绝对寻址的 `jmp *abs32` 仍能给出证据。
    let socket = report
        .imports
        .iter()
        .find(|row| row.import == "socket")
        .expect("socket row");
    assert_eq!(socket.status, Ternary::True);
    assert_eq!(socket.sites.len(), 1);
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

#[test]
fn entry_file_offset_maps_through_pt_load() {
    // 官方 YARA 的 ELF entrypoint = **文件偏移**（yara-python 实测判定：
    // 3 个 ET_EXEC 判别样本 vaddr 全不匹配、offset 全匹配——与 PE 同语义）。
    // 夹具：e_entry=0x400100，PT_LOAD(offset=0, vaddr=0x400000) → 0x100。
    let bytes = support::elf_sample(false, true);
    let info = elf::parse(&bytes).expect("elf sample must parse");
    assert_eq!(info.entry, 0x40_0100);
    assert_eq!(info.entry_file_offset(), Some(0x100));

    // 端到端：入口处的 4 字节作针，`$e at entrypoint` 经上下文求值命中；
    // 无上下文（库直调不传）→ Unknown，绝不 False。
    let needle = bytes[0x100..0x104].to_vec();
    let hex_spec: Vec<String> = needle.iter().map(|b| format!("{b:02X}")).collect();
    let src = format!(
        "rule at_ep {{\n  strings:\n    $e = {{ {} }}\n  condition:\n    $e at entrypoint\n}}\n",
        hex_spec.join(" ")
    );
    let parsed = trit_re::yaraparse::parse_rules(&src).unwrap();
    let trit_re::yaraparse::RuleOutcome::Parsed(rule) = &parsed.rules[0] else {
        panic!("应为 Parsed");
    };
    use trit_re::sigscan::SigScan;
    let r =
        SigScan::new(std::slice::from_ref(rule)).scan_with_entrypoint(&bytes, false, Some(0x100));
    assert_eq!(r.rules[0].value, Ternary::True);
    let r = SigScan::new(std::slice::from_ref(rule)).scan(&bytes, false);
    assert_eq!(r.rules[0].value, Ternary::Unknown, "无上下文绝不 False");
}
