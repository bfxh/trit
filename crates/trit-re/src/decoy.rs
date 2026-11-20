//! 对抗性投毒的诱饵检测（BLA 形防御盾的指纹面）。
//!
//! 背景（2026-11-13，BLA-ADVERSARIAL-STUDY）：反逆向盾会**故意投放**
//! 触发检测规则的诱饵——canary 里放假的敏感 API 名、假 SGX/TPM 清单、
//! 高熵蜂蜜表。本模块识别**文档化的公开指纹**（v1 = PE `.enclave` 节，
//! BLA README 明载的 COFF 诱饵节），并把它记成观察标记。
//!
//! **不降权**：节名完全由攻击者控制，只凭一个 `.enclave` 就把真实的
//! `imports.inj.*` 从 True 改成 Unknown，等于给攻击者一个一行 patch 的
//! 规避开关（加一个空节即可）。诱饵判断需要比节名更强的证据；在当前
//! 证据模型里，真实导入表就是真实能力，不能凭节名抹掉。因此本模块只
//! 记录 `decoy.markers`，不再改写能力事实。
//!
//! 诚实边界：只覆盖指纹可判别的公开盾（v1 一项）；无指纹 ≠ 无投毒
//! （事实照旧，不新增弃权）；ELF 侧盾形态待真实样本（如实留白）。

use crate::pe::PeInfo;
use crate::witness::Facts;
use trit_core::value::Ternary;

/// 文档化的诱饵节名（BLA 3.x PE 盾：`.enclave` COFF 诱饵节）。
const DECOY_SECTIONS: &[&str] = &[".enclave"];

/// 扫描诱饵指纹；命中只记录 `decoy.markers`，不改写能力事实。在 `verdict::evaluate` 之前调用。
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
            "对抗性加固指纹：诱饵节 [{}]——只记录标记，不降权能力事实（节名可被攻击者伪造）",
            markers.join(", ")
        ),
    );
    notes.push(format!(
        "decoy：命中对抗性加固指纹（{}），仅记录标记；能力事实不降权",
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
    fn enclave_marker_is_recorded_but_does_not_downgrade() {
        let pe = pe_with(vec![sec(".text"), sec(".enclave")]);
        let mut facts = poisoned_facts();
        let mut notes = Vec::new();
        scan(&pe, &mut facts, &mut notes);
        assert_eq!(facts.get("decoy.markers"), Some(Ternary::True));
        // 修复回归：攻击者加一个空 `.enclave` 节，不得把真实导入能力降成 Unknown。
        assert_eq!(facts.get("imports.inj.alloc"), Some(Ternary::True));
        assert_eq!(facts.get("dynres.stack_strings"), Some(Ternary::True));
        assert!(notes.iter().any(|n| n.contains("不降权")));
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
