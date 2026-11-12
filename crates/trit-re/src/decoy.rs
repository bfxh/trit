//! 对抗性投毒的诱饵检测（BLA 形防御盾的指纹面）。
//!
//! 背景（2026-11-13，BLA-ADVERSARIAL-STUDY）：反逆向盾会**故意投放**
//! 触发检测规则的诱饵——canary 里放假的敏感 API 名、假 SGX/TPM 清单、
//! 高熵蜂蜜表。本模块识别**文档化的公开指纹**（v1 = PE `.enclave` 节，
//! BLA README 明载的 COFF 诱饵节），命中时把注入/动态解析族的能力事实
//! **从 True 降权为 Unknown** 并注记——证据可能为投放诱饵，弃权而非
//! 假 -1（"扫描器永不 +1"在投毒对抗下的对称延伸：**诱饵面上的 True
//! 也不许当真**）。
//!
//! 诚实边界：只覆盖指纹可判别的公开盾（v1 一项）；无指纹 ≠ 无投毒
//! （事实照旧，不新增弃权）；ELF 侧盾形态待真实样本（如实留白）。

use crate::pe::PeInfo;
use crate::witness::Facts;
use trit_core::value::Ternary;

/// 文档化的诱饵节名（BLA 3.x PE 盾：`.enclave` COFF 诱饵节）。
const DECOY_SECTIONS: &[&str] = &[".enclave"];

/// 命中指纹时降权的能力事实族（诱饵最可能伪造的信号）。
const DOWNGRADE_IDS: &[&str] = &[
    "imports.inj.alloc",
    "imports.inj.write",
    "imports.inj.exec",
    "fn.injection_cooccur",
    "fn.injection_ordered",
    "dynres.hidden_api_strings",
    "dynres.stack_strings",
];

/// 扫描诱饵指纹；命中则降权能力事实。在 `verdict::evaluate` 之前调用。
pub fn scan(pe: &PeInfo, facts: &mut Facts, notes: &mut Vec<String>) {
    let markers: Vec<&str> = pe
        .sections
        .iter()
        .filter(|s| DECOY_SECTIONS.contains(&s.name.as_str()))
        .map(|s| s.name.as_str())
        .collect();
    if markers.is_empty() {
        return; // 无指纹：事实照旧——不新增弃权（无指纹 ≠ 无投毒，如实）
    }
    facts.set(
        "decoy.markers",
        Ternary::True,
        format!(
            "对抗性加固指纹：诱饵节 [{}]——能力证据可能为投放诱饵（BLA 形盾）",
            markers.join(", ")
        ),
    );
    for id in DOWNGRADE_IDS {
        if facts.get(id) == Some(Ternary::True) {
            facts.set(
                id,
                Ternary::Unknown,
                "诱饵面降权：命中对抗性加固指纹，能力证据可能为投放诱饵——弃权（原 True 见历史注记）",
            );
        }
    }
    notes.push(format!(
        "decoy：命中对抗性加固指纹（{}），能力事实已降权弃权",
        markers.join(", ")
    ));
}

#[cfg(test)]
mod tests {
    use super::scan;
    use crate::pe::{PeInfo, Section};
    use crate::witness::Facts;
    use trit_core::value::Ternary;

    fn pe_with(sections: Vec<Section>) -> PeInfo {
        PeInfo {
            plus: true,
            machine: 0xAA64,
            timestamp: 0,
            characteristics: 0x2102,
            subsystem: 3,
            entry_rva: 0x1000,
            image_base: 0x1_4000_0000,
            sections,
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
        }
    }

    fn sec(name: &str) -> Section {
        Section {
            name: name.into(),
            virtual_size: 0x1000,
            virtual_address: 0x1000,
            raw_size: 0x200,
            raw_ptr: 0x400,
            characteristics: 0x6000_0020,
            entropy: None,
        }
    }

    fn poisoned_facts() -> Facts {
        let mut f = Facts::default();
        f.set("imports.inj.alloc", Ternary::True, "诱饵");
        f.set("dynres.stack_strings", Ternary::True, "诱饵");
        f
    }

    #[test]
    fn enclave_marker_downgrades_capability_facts() {
        let pe = pe_with(vec![sec(".text"), sec(".enclave")]);
        let mut facts = poisoned_facts();
        let mut notes = Vec::new();
        scan(&pe, &mut facts, &mut notes);
        assert_eq!(facts.get("decoy.markers"), Some(Ternary::True));
        assert_eq!(facts.get("imports.inj.alloc"), Some(Ternary::Unknown));
        assert_eq!(facts.get("dynres.stack_strings"), Some(Ternary::Unknown));
        assert!(notes.iter().any(|n| n.contains("decoy")));
    }

    #[test]
    fn no_marker_leaves_facts_untouched() {
        let pe = pe_with(vec![sec(".text"), sec(".rdata")]);
        let mut facts = poisoned_facts();
        let mut notes = Vec::new();
        scan(&pe, &mut facts, &mut notes);
        assert_eq!(facts.get("decoy.markers"), None);
        assert_eq!(facts.get("imports.inj.alloc"), Some(Ternary::True));
        assert!(notes.is_empty());
    }

    #[test]
    fn unknown_facts_stay_unknown() {
        let pe = pe_with(vec![sec(".enclave")]);
        let mut facts = poisoned_facts();
        facts.set("imports.inj.write", Ternary::Unknown, "本来就弃权");
        let mut notes = Vec::new();
        scan(&pe, &mut facts, &mut notes);
        assert_eq!(facts.get("imports.inj.write"), Some(Ternary::Unknown));
    }
}
