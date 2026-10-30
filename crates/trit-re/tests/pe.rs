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
    assert_eq!(info.sections.len(), 3);
    assert_eq!(info.sections[0].name, ".text");
    assert!(info.sections[0].executable());

    let dlls = info.imports.as_ref().expect("imports parsed");
    assert_eq!(dlls.len(), 1);
    assert_eq!(dlls[0].name, "KERNEL32.dll");
    assert_eq!(dlls[0].functions.len(), 2);
    assert_eq!(dlls[0].functions[0].name, "LoadLibraryA");
    assert_eq!(dlls[0].functions[0].slot_rva, Some(0x2040));
    assert_eq!(dlls[0].functions[1].name, "connect");
    assert_eq!(dlls[0].functions[1].slot_rva, Some(0x2048));
    let pdata = info.runtime_functions.as_ref().expect("pdata parsed");
    assert_eq!(pdata.len(), 1);
    assert_eq!((pdata[0].begin, pdata[0].end), (0x1000, 0x1030));

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
    assert_eq!(triage.facts.get("imports.network"), Some(Ternary::True));
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
    let loader = report
        .imports
        .iter()
        .find(|row| row.import == "KERNEL32.dll!LoadLibraryA")
        .expect("loader row");
    assert_eq!(loader.status, Ternary::True);
    assert_eq!(loader.sites.len(), 1);
    assert_eq!(loader.sites[0].addr, 0x1010);
    assert_eq!(loader.sites[0].kind, trit_re::SiteKind::Call);
    let network = report
        .imports
        .iter()
        .find(|row| row.import == "KERNEL32.dll!connect")
        .expect("network row");
    assert_eq!(network.status, Ternary::True);
    assert_eq!(network.sites[0].addr, 0x1020);
    // 事实与判定都要能吃到交叉引用结果。
    assert_eq!(triage.facts.get("xref.scan_complete"), Some(Ternary::True));
    let gaps = triage
        .judgments
        .iter()
        .find(|j| j.id == "xref_gaps")
        .unwrap();
    assert_eq!(gaps.value, Ternary::False);

    // 两个调用点都在 .pdata 的 0x1000..0x1030 里 ⇒ 同函数共现可判定。
    assert_eq!(triage.facts.get("fn.exact_boundaries"), Some(Ternary::True));
    assert_eq!(triage.facts.get("fn.stager_cooccur"), Some(Ternary::True));
    assert_eq!(
        triage.facts.get("fn.injection_cooccur"),
        Some(Ternary::False)
    );
    let stager = triage
        .judgments
        .iter()
        .find(|j| j.id == "fn_stager")
        .unwrap();
    assert_eq!(stager.value, Ternary::True);
    let injection = triage
        .judgments
        .iter()
        .find(|j| j.id == "fn_injection")
        .unwrap();
    assert_eq!(injection.value, Ternary::False);
    let functions = triage.functions.as_ref().expect("function report");
    assert!(functions.exact);
    assert_eq!(functions.unattributed_sites, 0);
}

#[test]
fn parses_clr_and_tls_facts() {
    let bytes = support::pe_sample();
    let info = pe::parse(&bytes).expect("parse");
    let clr = info.clr.as_ref().expect("clr header");
    assert_eq!(clr.runtime_version.as_deref(), Some("v4.0.30319"));
    assert_eq!(clr.flags, 1);
    assert_eq!(clr.entry_token, 0x0600_0001);
    assert!(info.clr_dir_present);
    assert!(info.tls_dir_present);
    assert_eq!(
        info.tls_callbacks.as_deref(),
        Some([0x1010u32, 0x1020].as_slice())
    );

    let triage = analyze(&bytes);
    assert_eq!(triage.facts.get("pe.managed"), Some(Ternary::True));
    assert_eq!(triage.facts.get("pe.tls_callbacks"), Some(Ternary::True));
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

/// 真实的 Authenticode 证书位于**所有节区之后**，它不是叠加段。
///
/// `pe_sample()` 把证书摆在 0x7A0（`.rdata` 内部），而真实布局里它必须在文件尾部；
/// 正因为样本不真实，这个误报才一直没被测出来。
#[test]
fn authenticode_certificate_is_not_overlay() {
    let mut bytes = support::pe_sample();
    // 节区在 0xC00 结束。截到那里，按真实布局重新追加证书，再追加真正的尾巴。
    let cert_start = 0xC00usize;
    let cert_len = 0x60usize;
    let trailing = 0x400usize;
    bytes.truncate(cert_start);
    bytes.resize(cert_start + cert_len, 0xAA);
    bytes.resize(cert_start + cert_len + trailing, 0);
    for (i, slot) in bytes[cert_start + cert_len..].iter_mut().enumerate() {
        *slot = (i * 37 % 251) as u8;
    }
    // PE32+ 可选头 = 0x84 + 20 = 0x98；数据目录基址 = 0x98 + 112 = 0x108；
    // 安全目录是第 4 项 = 0x108 + 4 * 8 = 0x128。
    support::put_u32(&mut bytes, 0x128, cert_start as u32);
    support::put_u32(&mut bytes, 0x12C, cert_len as u32);

    let info = pe::parse(&bytes).expect("synthetic PE must parse");
    assert!(info.has_security_dir, "证书目录必须被认出来");
    let overlay = info.overlay.expect("证书之后的 1 KiB 尾巴是叠加段");
    assert_eq!(
        overlay.offset,
        cert_start + cert_len,
        "叠加段必须从证书之后开始"
    );
    assert_eq!(overlay.size, trailing, "证书的 0x60 字节不得计入叠加段");

    // 证书是文件最后一块时，叠加段应为空。
    bytes.truncate(cert_start + cert_len);
    let info = pe::parse(&bytes).expect("synthetic PE must still parse");
    assert!(info.has_security_dir);
    assert!(info.overlay.is_none(), "只有证书、没有尾巴时不应报叠加段");
}
