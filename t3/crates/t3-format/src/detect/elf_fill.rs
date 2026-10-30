//! ELF 侧的事实填充。
//!
//! 从 `detect.rs` 拆出，守住单文件 800 行预算。

use t3_core::{FactId, Facts};
use t3_kernel::entropy;

use crate::elf::ElfInfo;

use crate::pe::{self, cap};

use super::{
    calibration, presence, set_entropy_facts, DYNRESOLVE_IMPORT_MAX, MIN_ENTROPY_BYTES,
    OVERLAY_DOMINANT_PERCENT, OVERLAY_ENTROPY,
};

pub(super) fn fill_elf(facts: &mut Facts, info: &ElfInfo<'_>) {
    facts.set_bool(FactId::FormatElf, true, "ELF 魔数");
    facts.set_bool(FactId::FormatPe, false, "非 PE");

    let stats = info.entropy_stats(MIN_ENTROPY_BYTES);
    set_entropy_facts(facts, stats.max_exec, stats.max_data, stats.unreadable);

    let mut packer_hit = false;
    for (i, _) in info.sections().iter().enumerate() {
        if let Some(name) = info.section_name(i) {
            if pe::is_packer_section(name) {
                packer_hit = true;
            }
        }
    }
    facts.set_bool(FactId::SectionPackerName, packer_hit, "壳/保护器节名");
    facts.set_bool(FactId::ElfWritableText, info.writable_text, "可写可执行段");
    facts.set_bool(
        FactId::SectionWritableExecutable,
        info.writable_text,
        "可写可执行段",
    );
    facts.set_bool(FactId::ElfPie, info.is_pie(), "ET_DYN");
    if let Some(nx) = info.nx_stack {
        facts.set_bool(FactId::ElfNxStack, nx, "PT_GNU_STACK 不可执行");
    } else {
        facts.set_unknown(FactId::ElfNxStack, "无 PT_GNU_STACK");
    }
    facts.set_bool(FactId::ElfBuildId, info.build_id.is_some(), "GNU build-id");
    facts.set_bool(FactId::ElfStripped, !info.has_symtab, "无 SHT_SYMTAB");
    facts.set_bool(
        FactId::SectionMalformed,
        info.declared_section_count > info.section_count,
        "节头表被截断",
    );
    facts.set_bool_at(
        FactId::SectionCountAnomalous,
        info.declared_section_count == 0 || info.declared_section_count > 64,
        info.declared_section_count as f32,
        "节区数量",
    );

    // Imports: dynamic symbols.
    let imports = &info.imports;
    facts.set_bool(
        FactId::ImportsReadable,
        imports.complete,
        "动态符号表完整读取",
    );
    let has_code = info
        .segments()
        .iter()
        .any(|s| s.flags & 1 != 0 && s.filesz >= calibration::MIN_CODE_BYTES as u64);
    facts.set_bool(
        FactId::ImportsAbsent,
        !imports.present && has_code,
        "有代码但无动态符号表",
    );
    facts.set_unknown(FactId::ImportsSparse, "ELF 导入稀疏度未标定");
    for (id, bit) in [
        (FactId::ImportsLoader, cap::LOADER),
        (FactId::ImportsNetwork, cap::NETWORK),
        (FactId::ImportsCredential, cap::CREDENTIAL),
        (FactId::ImportsAntiDebug, cap::ANTI_DEBUG),
        (FactId::ImportsProcessEnum, cap::PROCESS_ENUM),
        (FactId::ImportsCrypto, cap::CRYPTO),
        (FactId::ImportsPersistence, cap::PERSISTENCE),
    ] {
        facts.set(
            id,
            presence(imports.has(bit), imports.complete),
            f32::NAN,
            "动态符号能力面",
        );
    }
    let trio = imports.caps & (cap::ALLOC_REMOTE | cap::WRITE_REMOTE | cap::EXEC_REMOTE)
        == (cap::ALLOC_REMOTE | cap::WRITE_REMOTE | cap::EXEC_REMOTE);
    facts.set(
        FactId::ImportsInjectionTrio,
        presence(trio, imports.complete),
        f32::NAN,
        "远程注入三件套",
    );
    facts.set(
        FactId::ImportsDynamicResolveOnly,
        presence(
            imports.has(cap::DYNAMIC_RESOLVE) && imports.function_count <= DYNRESOLVE_IMPORT_MAX,
            imports.complete,
        ),
        f32::NAN,
        "动态解析且静态导入稀疏",
    );

    // Overlay: an ELF file with bytes past the last section is unusual.
    match info.overlay() {
        Some(region) if region.data.len() >= MIN_ENTROPY_BYTES => {
            facts.set_bool_at(
                FactId::OverlayPresent,
                true,
                region.data.len() as f32,
                "叠加段字节数",
            );
            facts.set_band(
                FactId::OverlayHighEntropy,
                entropy(region.data) as f32,
                OVERLAY_ENTROPY,
                "叠加段熵",
            );
            let percent = (region.data.len() * 100) / info.bytes.len().max(1);
            facts.set_bool_at(
                FactId::OverlayDominant,
                percent >= OVERLAY_DOMINANT_PERCENT,
                percent as f32,
                "叠加段占文件百分比",
            );
        }
        Some(_) => {
            facts.set_bool(FactId::OverlayPresent, true, "叠加段过小");
            facts.set_unknown(FactId::OverlayHighEntropy, "叠加段过小");
            facts.set_bool(FactId::OverlayDominant, false, "叠加段过小");
        }
        None => {
            facts.set_bool(FactId::OverlayPresent, false, "无叠加段");
            facts.set_bool(FactId::OverlayHighEntropy, false, "无叠加段");
            facts.set_bool(FactId::OverlayDominant, false, "无叠加段");
        }
    }

    // Facts that only make sense for PE stay Unknown for ELF, which keeps the
    // verdict interval wide instead of inventing a "clean" reading.
    for id in [
        FactId::SignedPresent,
        FactId::PdbPresent,
        FactId::PdbDeveloperPath,
        FactId::RichHeaderValid,
        FactId::DotnetPresent,
        FactId::TlsCallbacks,
        FactId::EntryOutsideCode,
        FactId::EntryInLastSection,
        FactId::TimestampAnomalous,
        FactId::LayoutNonMonotone,
    ] {
        facts.set_unknown(id, "ELF 不适用");
    }
    facts.set_bool(FactId::RelocationsPresent, true, "ELF 使用重定位");
    facts.set_bool(FactId::ResourcesPresent, false, "ELF 无资源目录");
}
