//! 安全裁决层：把 `Trit` 落成 DeltaDB 合并门禁的三种动作与红队证据结构。

use crate::dag::NodeId;
use crate::trit::Trit;
use std::fmt;

/// 合并门禁的三态动作——恰好也是三进制：放行 / 冻结 / 阻断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// 全链 +1：可合并。
    Allow,
    /// 存在 0 态：冻结合并，送沙盒或等人裁。
    Freeze,
    /// 存在 -1：已打穿，禁止合并。
    Block,
}

impl GateDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "ALLOW",
            Self::Freeze => "FREEZE",
            Self::Block => "BLOCK",
        }
    }
}

impl fmt::Display for GateDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 攻击向量（红队的攻击类型标签）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttackVector {
    SqlInjection,
    Xss,
    AuthBypass,
    PromptInjection,
    FuzzCrash,
    Other(String),
}

impl fmt::Display for AttackVector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SqlInjection => f.write_str("SQL注入"),
            Self::Xss => f.write_str("XSS"),
            Self::AuthBypass => f.write_str("鉴权绕过"),
            Self::PromptInjection => f.write_str("Prompt注入"),
            Self::FuzzCrash => f.write_str("模糊崩溃"),
            Self::Other(name) => write!(f, "{name}"),
        }
    }
}

/// 攻击成功的证据：作为"负数消息"挂在攻击节点上，`target` 回链到被打穿的代码节点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExploitEvidence {
    pub vector: AttackVector,
    /// 触发漏洞的恶意输入。
    pub payload: String,
    /// 被打穿的代码/消息节点。
    pub target: NodeId,
    /// 第几轮攻击得手。
    pub round: u32,
}

/// 合并门禁 = Kleene AND 折叠：
/// 任一 `-1` → `BLOCK`；否则任一 `0` → `FREEZE`；全 `+1` → `ALLOW`。
pub fn merge_gate(verdicts: impl IntoIterator<Item = Trit>) -> GateDecision {
    match Trit::fold_and(verdicts) {
        Trit::Vulnerable => GateDecision::Block,
        Trit::Unknown => GateDecision::Freeze,
        Trit::Hardened => GateDecision::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_is_kleene_and() {
        assert_eq!(merge_gate([Trit::Hardened, Trit::Hardened]), GateDecision::Allow);
        assert_eq!(merge_gate([Trit::Hardened, Trit::Unknown]), GateDecision::Freeze);
        assert_eq!(merge_gate([Trit::Unknown, Trit::Vulnerable]), GateDecision::Block);
    }
}
