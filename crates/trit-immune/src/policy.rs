//! 合并策略引擎：偏好档位、分层 CI、预测合并——全部三进制表达。
//!
//! 对应平台愿景：**绿色自动合并（按通道显式开启，永不默认）**、实验室/封存态、
//! 测试基准门槛、**分层 CI（这层过了才扫下一层，永不一次性放行）**、
//! **预测合并**（输入 = 合并门禁 + 传播后有效裁决——预测器就是免疫网络本身）。
//! 纯函数、可复算、无副作用：智能体通道与调度器只消费 `decide` 的结果，不需要 LLM。

use crate::trit::Trit;
use crate::verdict::GateDecision;

/// 通道状态：主链 / 实验室 / 封存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// 主链：绿色合并开关可用（仍需显式开启）。
    Main,
    /// 实验室：永不自动合并，跑完也只 Hold。
    Lab,
    /// 封存：只读，不接受合并。
    Closed,
}

/// 合并偏好（按 PR/通道显式配置）。
#[derive(Debug, Clone)]
pub struct MergePolicy {
    pub channel: Channel,
    /// 绿色合并开关：全部通过时是否自动合并。
    pub auto_merge: bool,
    /// 要求的测试基准名（未达标即冻结）。
    pub required_baselines: Vec<String>,
}

impl Default for MergePolicy {
    fn default() -> Self {
        // 默认保守：主链、不开自动合并、无基准要求
        Self {
            channel: Channel::Main,
            auto_merge: false,
            required_baselines: Vec::new(),
        }
    }
}

/// 一次 PR 的快照（预测合并的输入）。
#[derive(Debug, Clone)]
pub struct PrSnapshot {
    /// 合并门禁（因果链 + 传播后有效裁决的 K3 AND 折叠）
    pub gate: GateDecision,
    /// 各 CI 层裁决：已通过 `+1` / 未通过 `-1` / 未跑 `0`
    pub layers: Vec<Trit>,
    /// 已通过的测试基准名
    pub baselines: Vec<String>,
    /// 传播后的有效裁决。`-1` **同样一票否决**（跨仓边/依赖坐标打进来的污染
    /// 不在 `gate` 的折叠范围内，只看 gate 会 fail-open）
    pub verdict: Trit,
}

/// Hold 携带的**机器可执行动作**：智能体循环消费它 → 单调状态前进一步 →
/// 有界终止（见 `docs/GOAL.md` 药方三）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 去跑 CI 第 n 层（结果 ±1 入账）。
    RunLayer(usize),
    /// 补测试基准。
    AddBaseline(String),
    /// 人工复核（0 态唯一出口：解除隔离或确证）。
    HumanReview,
    /// 开启自动合并（绿色合并未开，一次性开关）。
    EnableAutoMerge,
}

/// 策略裁决：自动合并 / 挂起（携带动作）/ 阻断（含原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    AutoMerge,
    Hold { action: Action, reason: String },
    Block(String),
}

/// 预测合并：由快照与偏好算出决策。判定顺序即严格流程：
/// 封存 → 门禁一票否决 → 分层 CI → 测试基准 → 冻结态 → 绿色开关。
/// 每个 Hold 的动作都使单调状态前进一步 ⇒ 循环有界终止。
pub fn decide(policy: &MergePolicy, pr: &PrSnapshot) -> Decision {
    if policy.channel == Channel::Closed {
        return Decision::Hold {
            action: Action::HumanReview,
            reason: "封存通道：不接受合并".into(),
        };
    }
    if pr.gate == GateDecision::Block {
        return Decision::Block("因果链存在确证漏洞".into());
    }
    // 有效裁决（含传播/坐标污染）为 -1 时同样一票否决：
    // merge_gate 折的是因果链 + 锚定攻击分支，**看不见**跨仓边与依赖坐标打进来的
    // 污染；只看 gate 会让"库被打穿、本仓因果链干净"的 PR 直接自动合并（fail-open）。
    if pr.verdict == Trit::Vulnerable {
        return Decision::Block("有效裁决为 -1（传播或依赖坐标污染）".into());
    }
    for (i, l) in pr.layers.iter().enumerate() {
        match l {
            Trit::Vulnerable => return Decision::Block(format!("CI 第 {} 层未通过", i + 1)),
            Trit::Unknown => {
                return Decision::Hold {
                    action: Action::RunLayer(i + 1),
                    reason: format!("CI 第 {} 层待跑", i + 1),
                }
            }
            Trit::Hardened => {}
        }
    }
    for b in &policy.required_baselines {
        if !pr.baselines.iter().any(|x| x == b) {
            return Decision::Hold {
                action: Action::AddBaseline(b.clone()),
                reason: format!("缺测试基准：{b}"),
            };
        }
    }
    if pr.gate == GateDecision::Freeze || pr.verdict == Trit::Unknown {
        return Decision::Hold {
            action: Action::HumanReview,
            reason: "存在未知面或隔离未解除".into(),
        };
    }
    match policy.channel {
        Channel::Lab => Decision::Hold {
            action: Action::HumanReview,
            reason: "实验室通道：永不自动合并".into(),
        },
        Channel::Main if policy.auto_merge => Decision::AutoMerge,
        Channel::Main => Decision::Hold {
            action: Action::EnableAutoMerge,
            reason: "全部通过，但自动合并未开启（绿色合并按通道显式开启）".into(),
        },
        Channel::Closed => unreachable!("封存通道已在开头返回"),
    }
}

/// 分层 CI 调度：返回第一个待跑层的下标（全跑完 → `None`）。
pub fn next_layer(layers: &[Trit]) -> Option<usize> {
    layers.iter().position(|l| *l == Trit::Unknown)
}

#[cfg(test)]
mod tests;
