//! 动作级 capability：把「读外部文字」与「会产生副作用的动作」分成两层。
//!
//! 入口文字闸只降低「数据被当指令」的概率；这一层回答另一个问题：
//! **即使模型被骗，它能不能真的做出高权限动作。** 策略是三值的：
//! - `True`：类别已显式授予，且不是由不可信数据触发；
//! - `Unknown`：类别授予了，但动作由不可信数据触发——需要人确认或一次性提权；
//! - `False`：类别没授予。读之外一律默认拒绝。
//!
//! 边界（如实）：这是**策略内核**；per-target 白名单、以及 `tools/sandbox_run.py` /
//! `tools/subagent_dispatch.py` 的调用面接入尚未做，不能把它当端到端能力防护。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::Ternary;

/// 动作类别。`Read` 不改变外部状态；其余三类都会产生副作用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    /// 读文件、网页、工具输出——不改变外部状态。
    Read,
    /// 写文件、改配置、提交/推送。
    Write,
    /// 执行命令、启动进程。
    Exec,
    /// 发网络请求、上传数据。
    Network,
}

impl ActionClass {
    /// 是否会改变外部状态。
    pub fn is_side_effecting(self) -> bool {
        !matches!(self, Self::Read)
    }
}

/// 一条待判定的动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSpec {
    pub class: ActionClass,
    /// 动作是否由**不可信数据**（网页 / issue / 工具输出 / 子智能体产物）触发。
    #[serde(default)]
    pub from_untrusted_data: bool,
}

/// 显式授予的动作类别；不给就是拒绝。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    #[serde(default)]
    pub allow: BTreeSet<ActionClass>,
    /// 是否允许不可信数据触发写/执行/网络（默认 false → 判 `Unknown`）。
    #[serde(default)]
    pub allow_untrusted_origin: bool,
}

impl CapabilityPolicy {
    /// 只读策略：读放行，三类副作用动作全拒。
    pub fn read_only() -> Self {
        Self::default()
    }
}

/// 判定理由，写进回执而不是只给一个布尔。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityReason {
    /// `Read`：不改状态，放行。
    ReadOnly,
    /// 类别已授予，来源可信。
    Granted,
    /// 类别已授予，且策略允许不可信来源。
    GrantedForUntrustedOrigin,
    /// 类别没在 `allow` 里。
    ClassNotGranted,
    /// 类别授予了，但不可信来源需要人确认。
    UntrustedOriginRequiresConfirmation,
}

/// 一次判定：三值结论 + 理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDecision {
    pub action: ActionClass,
    pub verdict: Ternary,
    pub reason: CapabilityReason,
}

/// 只算结论，给 admission 链用。
pub fn capability_gate(policy: &CapabilityPolicy, action: &ActionSpec) -> Ternary {
    decide(policy, action).verdict
}

/// 完整判定（带理由）。
pub fn decide(policy: &CapabilityPolicy, action: &ActionSpec) -> CapabilityDecision {
    let class = action.class;
    let (verdict, reason) = if class == ActionClass::Read {
        (Ternary::True, CapabilityReason::ReadOnly)
    } else if !policy.allow.contains(&class) {
        (Ternary::False, CapabilityReason::ClassNotGranted)
    } else if action.from_untrusted_data && !policy.allow_untrusted_origin {
        (
            Ternary::Unknown,
            CapabilityReason::UntrustedOriginRequiresConfirmation,
        )
    } else if action.from_untrusted_data {
        (Ternary::True, CapabilityReason::GrantedForUntrustedOrigin)
    } else {
        (Ternary::True, CapabilityReason::Granted)
    };
    CapabilityDecision {
        action: class,
        verdict,
        reason,
    }
}

/// 第六门：动作级 capability。与五门做 K3 AND：任一门 False 即拒绝，
/// `Unknown`（不可信来源的写/执行/网络）保持 `Unknown`，留给人工确认。
pub fn admit_with_capability(
    dependencies: Ternary,
    authorization: Ternary,
    resources: Ternary,
    freshness: Ternary,
    claim: Ternary,
    capability: Ternary,
) -> Ternary {
    crate::admission::admit_with_claim(dependencies, authorization, resources, freshness, claim)
        & capability
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(class: ActionClass, tainted: bool) -> ActionSpec {
        ActionSpec {
            class,
            from_untrusted_data: tainted,
        }
    }

    fn policy(classes: &[ActionClass], allow_untrusted_origin: bool) -> CapabilityPolicy {
        CapabilityPolicy {
            allow: classes.iter().copied().collect(),
            allow_untrusted_origin,
        }
    }

    #[test]
    fn read_is_allowed_even_from_untrusted_data() {
        let decision = decide(&policy(&[], false), &spec(ActionClass::Read, true));
        assert_eq!(decision.verdict, Ternary::True);
        assert_eq!(decision.reason, CapabilityReason::ReadOnly);
    }

    #[test]
    fn side_effecting_classes_are_denied_by_default() {
        for class in [ActionClass::Write, ActionClass::Exec, ActionClass::Network] {
            let decision = decide(&CapabilityPolicy::read_only(), &spec(class, false));
            assert_eq!(decision.verdict, Ternary::False, "{class:?}");
            assert_eq!(decision.reason, CapabilityReason::ClassNotGranted);
        }
    }

    #[test]
    fn granted_side_effect_is_true_when_trusted() {
        let decision = decide(
            &policy(&[ActionClass::Write], false),
            &spec(ActionClass::Write, false),
        );
        assert_eq!(decision.verdict, Ternary::True);
        assert_eq!(decision.reason, CapabilityReason::Granted);
    }

    #[test]
    fn untrusted_origin_turns_grant_into_confirmation() {
        let decision = decide(
            &policy(&[ActionClass::Network], false),
            &spec(ActionClass::Network, true),
        );
        assert_eq!(decision.verdict, Ternary::Unknown);
        assert_eq!(
            decision.reason,
            CapabilityReason::UntrustedOriginRequiresConfirmation
        );
    }

    #[test]
    fn explicit_untrusted_grant_is_true_and_reported() {
        let decision = decide(
            &policy(&[ActionClass::Exec], true),
            &spec(ActionClass::Exec, true),
        );
        assert_eq!(decision.verdict, Ternary::True);
        assert_eq!(decision.reason, CapabilityReason::GrantedForUntrustedOrigin);
    }

    #[test]
    fn gate_matches_decide() {
        let policy = policy(&[ActionClass::Write], false);
        assert_eq!(
            capability_gate(&policy, &spec(ActionClass::Write, true)),
            Ternary::Unknown
        );
        assert_eq!(
            capability_gate(&policy, &spec(ActionClass::Network, false)),
            Ternary::False
        );
    }

    #[test]
    fn policy_parses_from_json_and_refuses_unknown_class() {
        let parsed: CapabilityPolicy =
            serde_json::from_str(r#"{"allow":["read","write"],"allow_untrusted_origin":false}"#)
                .expect("policy");
        assert!(parsed.allow.contains(&ActionClass::Write));
        assert!(!parsed.allow_untrusted_origin);

        let bad = serde_json::from_str::<ActionSpec>(r#"{"class":"shell"}"#);
        assert!(bad.is_err(), "未知动作类别必须在解析层拒绝");
    }
}
