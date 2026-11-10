//! The default hypothesis set.
//!
//! # How the aggregate is computed, and why it is not a score
//!
//! Each hypothesis contributes its own [`Grade`], and the file's verdict is the
//! **maximum** over contributions — that is, the join in the grade lattice
//! `Clean < Suspicious < Malicious`. Joins are associative, commutative and
//! idempotent, which buys three properties that additive scoring does not have:
//!
//! 1. **Duplicate evidence cannot escalate.** A detector that fires twice, or a
//!    fact that supports two hypotheses, contributes once per hypothesis and never
//!    compounds. Score sums let a noisy detector manufacture severity.
//! 2. **Order independence is structural.** Shuffling the hypothesis list cannot
//!    change the answer, so parallel evaluation is safe without a merge rule.
//! 3. **No arbitrary calibration.** A sum of "weights" needs a threshold at the
//!    end, and that threshold is where the fake precision lives. A join needs no
//!    threshold because the grades are already meaningful.
//!
//! The cost is that corroboration is not rewarded: three independent
//! `Suspicious` findings still read as `Suspicious`. That is deliberate. The
//! interval `[lo, hi]` is where the tool expresses "how sure", and it does so
//! using evidence that is *missing* rather than evidence that is merely
//! duplicated.
//!
//! # Choosing rules
//!
//! Precision is uneven across these hypotheses and the severities say so. The
//! `Malicious` ones require a *combination* of APIs that has no benign
//! explanation (remote allocate + remote write + remote execute; runtime load +
//! network). The `Suspicious` ones are structural and cheap to hit — a packed
//! benign installer will land there, and the `[lo, hi]` interval plus the
//! counterfactual list is the honest way to present that.

use crate::expr::Expr;
use crate::fact::FactId;
use crate::verdict::{Engine, Format, Grade, Hypothesis};

/// `f(x, y)` shorthand.
fn both(a: FactId, b: FactId) -> Expr {
    Expr::fact(a).and(Expr::fact(b))
}

/// `f(x) ∨ f(y)` shorthand.
fn either(a: FactId, b: FactId) -> Expr {
    Expr::fact(a).or(Expr::fact(b))
}

/// The built-in rule set.
///
/// Identifiers are a wire contract: they appear in JSONL output and in
/// counterfactual explanations, so renaming one is a breaking change.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "单张规则表：按严重度分组更便于逐条核对，拆成多个函数会把分组信息拆散"
)]
pub fn default_engine() -> Engine {
    Engine::new(vec![
        // ---------------------------------------------------------------
        // Malicious: capability combinations with no common benign reading.
        // ---------------------------------------------------------------
        // Capability alone is not malice. A debugger, a browser's sandbox broker and
        // Windows' own `KernelBase.dll` all link the injection primitives by design; the
        // first version of this rule set called all of them malicious. What is genuinely
        // rare is *capability on top of concealment*, so Malicious requires both and the
        // bare capability stays at Suspicious.
        Hypothesis::new(
            "injection_obfuscated",
            "注入能力 + 结构隐蔽（高熵或壳节名）",
            both(FactId::ImportsInjectionTrio, FactId::SectionHighEntropy).or(both(
                FactId::ImportsInjectionTrio,
                FactId::SectionPackerName,
            )),
            Grade::Malicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "credential_obfuscated",
            "凭据与网络能力 + 结构隐蔽",
            both(FactId::ImportsCredential, FactId::ImportsNetwork).and(
                Expr::fact(FactId::SectionHighEntropy).or(Expr::fact(FactId::SectionPackerName)),
            ),
            Grade::Malicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "sig_high",
            "命中高危签名",
            Expr::fact(FactId::SigHigh),
            Grade::Malicious,
        ),
        // ---------------------------------------------------------------
        // Suspicious: strong structural signals, individually ambiguous.
        // ---------------------------------------------------------------
        Hypothesis::new(
            "injection_capability",
            "具备远程注入能力（申请 + 写入 + 执行）",
            Expr::fact(FactId::ImportsInjectionTrio),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "credential_exfil",
            "凭据访问 API + 网络能力",
            both(FactId::ImportsCredential, FactId::ImportsNetwork),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "staged_loader",
            "分阶段装载：运行期解析 API + 网络能力",
            both(FactId::ImportsLoader, FactId::ImportsNetwork),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "packed",
            "可执行节区高熵、壳节名或高熵叠加段",
            Expr::any_of(vec![
                Expr::fact(FactId::SectionHighEntropy),
                Expr::fact(FactId::SectionPackerName),
                Expr::fact(FactId::OverlayHighEntropy),
            ]),
            Grade::Suspicious,
        )
        // A valid Authenticode signature is an *explanation*, not a clearance:
        // signed-and-packed is the signature of a tampered or repurposed binary, and
        // that shape must surface as a conflict rather than being averaged away.
        .with_benign(Expr::fact(FactId::SignedPresent))
        .only(Format::Binary),
        Hypothesis::new(
            "embedded_payload",
            "叠加段占比大且高熵（内嵌载荷）",
            both(FactId::OverlayDominant, FactId::OverlayHighEntropy),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "data_blob",
            "非可执行节区高熵（内嵌加密块或压缩数据）",
            Expr::fact(FactId::SectionHighEntropyData),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "layout_anomaly",
            "节表/入口点布局异常",
            Expr::any_of(vec![
                Expr::fact(FactId::EntryOutsideCode),
                Expr::fact(FactId::EntryInLastSection),
                Expr::fact(FactId::LayoutNonMonotone),
                Expr::fact(FactId::SectionMalformed),
                Expr::fact(FactId::SectionCountAnomalous),
            ]),
            Grade::Suspicious,
        )
        .only(Format::Pe),
        Hypothesis::new(
            "writable_code",
            "存在可写可执行节区，或仅有虚拟大小的空壳节区",
            either(FactId::SectionWritableExecutable, FactId::SectionZeroRaw),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "self_defense",
            "反调试 + 进程枚举（反分析姿态）",
            both(FactId::ImportsAntiDebug, FactId::ImportsProcessEnum),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "dynamic_resolve",
            "只做动态解析而无对应静态导入",
            Expr::fact(FactId::ImportsDynamicResolveOnly),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "sparse_imports",
            "导入表相对于体积异常稀疏",
            Expr::fact(FactId::ImportsSparse),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "persistence",
            "引用服务/计划任务等持久化接口",
            Expr::fact(FactId::ImportsPersistence),
            Grade::Suspicious,
        )
        .only(Format::Binary),
        Hypothesis::new(
            "timestamp_anomaly",
            "PE 时间戳为零",
            Expr::fact(FactId::TimestampAnomalous),
            Grade::Suspicious,
        )
        .only(Format::Pe),
        Hypothesis::new(
            "sig_medium",
            "命中中等强度签名",
            Expr::fact(FactId::SigMedium),
            Grade::Suspicious,
        ),
        Hypothesis::new(
            "elf_writable_text",
            "ELF 代码段可写",
            Expr::fact(FactId::ElfWritableText),
            Grade::Suspicious,
        )
        .only(Format::Elf),
        Hypothesis::new(
            "elf_layout_anomaly",
            "ELF 节头表或节数量异常",
            Expr::any_of(vec![
                Expr::fact(FactId::SectionMalformed),
                Expr::fact(FactId::SectionCountAnomalous),
            ]),
            Grade::Suspicious,
        )
        .only(Format::Elf),
        // ---------------------------------------------------------------
        // Informational: reported, never graded.
        // ---------------------------------------------------------------
        Hypothesis::new(
            "dev_build",
            "携带开发者本机 PDB 路径（构建环境信息泄漏）",
            Expr::fact(FactId::PdbDeveloperPath),
            Grade::Clean,
        )
        .only(Format::Pe),
    ])
}

#[cfg(test)]
mod tests {
    use super::default_engine;
    use crate::fact::FactId;
    use crate::verdict::Grade;

    #[test]
    fn every_rule_id_is_unique_and_lowercase() {
        let engine = default_engine();
        let mut seen = std::collections::BTreeSet::new();
        for h in engine.hypotheses() {
            assert!(seen.insert(h.id), "duplicate rule id {}", h.id);
            assert!(
                h.id.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "rule id {} is not snake_case",
                h.id
            );
            assert_ne!(h.title, "");
        }
        assert!(engine.hypotheses().len() >= 12);
    }

    #[test]
    fn rules_only_reference_real_facts() {
        let engine = default_engine();
        for h in engine.hypotheses() {
            let refs = h.expr.referenced();
            assert!(!refs.is_empty(), "rule {} is unconditional", h.id);
            for id in refs {
                assert!(FactId::ALL.contains(&id));
            }
        }
    }

    #[test]
    fn malicious_rules_need_two_rails() {
        // Guard against a future edit that promotes a single weak fact to
        // Malicious and tanks precision. Two kinds of fact are exempt because they
        // are already conjunctions: signature hits (resolved byte by byte in the
        // matcher) and derived capability trios (computed from three separate
        // imports by the format layer).
        let engine = default_engine();
        for h in engine.hypotheses() {
            if h.severity != Grade::Malicious {
                continue;
            }
            let single_rail = h.expr.node_count() == 1;
            let already_conjunctive = h.expr.referenced().iter().any(|id| {
                matches!(
                    id,
                    FactId::SigHigh | FactId::SigMedium | FactId::ImportsInjectionTrio
                )
            });
            assert!(
                !single_rail || already_conjunctive,
                "rule {} raises to malicious on a single structural fact",
                h.id
            );
        }
    }
}
