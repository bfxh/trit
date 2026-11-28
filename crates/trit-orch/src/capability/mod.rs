//! 动作级 capability：把「读外部文字」与「会产生副作用的动作」分成两层。
//!
//! 入口文字闸只降低「数据被当指令」的概率；这一层回答另一个问题：
//! **即使模型被骗，它能不能真的做出高权限动作。** 策略是三值的：
//! - `True`：类别已显式授予，且不是由不可信数据触发；
//! - `Unknown`：类别授予了，但动作由不可信数据触发——需要人确认或一次性提权；
//! - `False`：类别没授予，或目标不在白名单。
//!
//! 目标白名单是**精确匹配**：不做路径规范化、域名小写化或通配；调用方要先把路径/域名归一，否则换写法会被拒（有意保守，不是漏洞）。
//! `Read` 不做目标白名单：读不改变状态，外传由 `Network` 门管。
//!
//! 边界（如实）：这是**策略内核**；`tools/sandbox_run.py` /
//! `tools/subagent_dispatch.py` 的调用面接入尚未做，不能把它当端到端能力防护。

use std::collections::{BTreeMap, BTreeSet};

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
    /// 目标（文件路径 / 域名 / 命令名）。类别只做类别级授予时可为 `None`；
    /// 策略一旦给该类别配了目标白名单，`None` 会被判 `False`。
    #[serde(default)]
    pub target: Option<String>,
    /// 动作是否由**不可信数据**（网页 / issue / 工具输出 / 子智能体产物）触发。
    #[serde(default)]
    pub from_untrusted_data: bool,
}

/// 显式授予的动作类别与目标；不给就是拒绝。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    #[serde(default)]
    pub allow: BTreeSet<ActionClass>,
    /// 是否允许不可信数据触发写/执行/网络（默认 false → 判 `Unknown`）。
    #[serde(default)]
    pub allow_untrusted_origin: bool,
    /// 类别 → 允许的目标集合。只有**列出的类别**才做目标级校验；
    /// 没列出的类别沿用类别级授予（v1 兼容）。空集合表示该类别任何目标都不允许。
    #[serde(default)]
    pub allow_targets: BTreeMap<ActionClass, BTreeSet<String>>,
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
    /// 类别已授予，来源可信，目标通过。
    Granted,
    /// 类别已授予，且策略允许不可信来源。
    GrantedForUntrustedOrigin,
    /// 类别没在 `allow` 里。
    ClassNotGranted,
    /// 该类别配了目标白名单，但动作没带目标。
    TargetRequired,
    /// 动作目标不在该类别白名单里。
    TargetNotAllowed,
    /// 类别授予了，但不可信来源需要人确认。
    UntrustedOriginRequiresConfirmation,
}

/// 一次判定：三值结论 + 理由 + 目标（便于回执审计）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDecision {
    pub action: ActionClass,
    #[serde(default)]
    pub target: Option<String>,
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
    } else if let Some(targets) = policy.allow_targets.get(&class) {
        match action.target.as_deref() {
            None => (Ternary::False, CapabilityReason::TargetRequired),
            Some(target) if !targets.contains(target) => {
                (Ternary::False, CapabilityReason::TargetNotAllowed)
            }
            Some(_) => origin_verdict(policy, action),
        }
    } else {
        origin_verdict(policy, action)
    };
    CapabilityDecision {
        action: class,
        target: action.target.clone(),
        verdict,
        reason,
    }
}

/// 目标通过后，再看动作是否来自不可信数据。
fn origin_verdict(policy: &CapabilityPolicy, action: &ActionSpec) -> (Ternary, CapabilityReason) {
    if action.from_untrusted_data && !policy.allow_untrusted_origin {
        (
            Ternary::Unknown,
            CapabilityReason::UntrustedOriginRequiresConfirmation,
        )
    } else if action.from_untrusted_data {
        (Ternary::True, CapabilityReason::GrantedForUntrustedOrigin)
    } else {
        (Ternary::True, CapabilityReason::Granted)
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
mod tests;
