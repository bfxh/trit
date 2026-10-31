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
    /// 传播后的有效裁决
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
mod tests {
    use super::*;

    fn green() -> PrSnapshot {
        PrSnapshot {
            gate: GateDecision::Allow,
            layers: vec![Trit::Hardened, Trit::Hardened],
            baselines: vec!["perf-baseline".into()],
            verdict: Trit::Hardened,
        }
    }

    #[test]
    fn green_main_with_switch_automerges() {
        let p = MergePolicy {
            channel: Channel::Main,
            auto_merge: true,
            required_baselines: vec!["perf-baseline".into()],
        };
        assert_eq!(decide(&p, &green()), Decision::AutoMerge);
    }

    #[test]
    fn green_without_switch_holds_with_enable_action() {
        let p = MergePolicy::default();
        match decide(&p, &green()) {
            Decision::Hold { action, reason } => {
                assert_eq!(action, Action::EnableAutoMerge);
                assert!(reason.contains("自动合并未开启"), "{reason}");
            }
            other => panic!("应 Hold，实际 {other:?}"),
        }
    }

    #[test]
    fn lab_never_automerges_even_if_green() {
        let p = MergePolicy {
            channel: Channel::Lab,
            auto_merge: true,
            required_baselines: vec![],
        };
        assert_eq!(
            decide(&p, &green()),
            Decision::Hold {
                action: Action::HumanReview,
                reason: "实验室通道：永不自动合并".into(),
            }
        );
    }

    #[test]
    fn closed_channel_is_read_only() {
        let p = MergePolicy {
            channel: Channel::Closed,
            auto_merge: true,
            required_baselines: vec![],
        };
        assert!(matches!(decide(&p, &green()), Decision::Hold { .. }));
    }

    #[test]
    fn gate_block_vetoes_everything() {
        let mut pr = green();
        pr.gate = GateDecision::Block;
        assert!(matches!(decide(&MergePolicy::default(), &pr), Decision::Block(_)));
    }

    #[test]
    fn layers_advance_one_at_a_time() {
        let p = MergePolicy::default();
        // 第 1 层未过 → 阻断
        let mut pr = green();
        pr.layers[0] = Trit::Vulnerable;
        assert!(matches!(decide(&p, &pr), Decision::Block(_)));
        // 第 2 层没跑 → 挂起，动作 = 去跑第 2 层
        let mut pr = green();
        pr.layers[1] = Trit::Unknown;
        match decide(&p, &pr) {
            Decision::Hold { action, reason } => {
                assert_eq!(action, Action::RunLayer(2));
                assert!(reason.contains("第 2 层待跑"), "{reason}");
            }
            other => panic!("应 Hold，实际 {other:?}"),
        }
        // 调度器：下一个待跑层
        assert_eq!(
            next_layer(&[Trit::Hardened, Trit::Unknown, Trit::Unknown]),
            Some(1)
        );
        assert_eq!(next_layer(&[Trit::Hardened; 3]), None);
    }

    #[test]
    fn missing_baseline_holds_with_add_action() {
        let p = MergePolicy {
            channel: Channel::Main,
            auto_merge: true,
            required_baselines: vec!["perf-baseline".into()],
        };
        let mut pr = green();
        pr.baselines.clear();
        match decide(&p, &pr) {
            Decision::Hold { action, reason } => {
                assert_eq!(action, Action::AddBaseline("perf-baseline".into()));
                assert!(reason.contains("perf-baseline"), "{reason}");
            }
            other => panic!("应 Hold，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_verdict_freezes_even_if_gate_allows() {
        let p = MergePolicy {
            channel: Channel::Main,
            auto_merge: true,
            required_baselines: vec![],
        };
        let mut pr = green();
        pr.verdict = Trit::Unknown;
        assert!(matches!(decide(&p, &pr), Decision::Hold { .. }));
    }

    #[test]
    fn agent_loop_terminates_within_bound() {
        // 智能体循环：decide → 执行 action → 重建快照 → 再 decide。
        // 状态单调（层每层至多变一次、基准集 ⊆ 策略固定集）⇒ 步数有界。
        let policy = MergePolicy {
            channel: Channel::Main,
            auto_merge: true,
            required_baselines: vec!["perf".into(), "fuzz".into()],
        };
        let mut pr = PrSnapshot {
            gate: GateDecision::Allow,
            layers: vec![Trit::Unknown; 3],
            baselines: vec![],
            verdict: Trit::Hardened,
        };
        let bound = pr.layers.len() + policy.required_baselines.len() + 1;
        let mut rounds = 0usize;
        loop {
            rounds += 1;
            assert!(rounds <= bound, "智能体循环必须在有界步内终止");
            match decide(&policy, &pr) {
                Decision::AutoMerge => break,
                Decision::Block(m) => panic!("本场景不应阻断：{m}"),
                Decision::Hold { action, .. } => match action {
                    Action::RunLayer(i) => pr.layers[i - 1] = Trit::Hardened,
                    Action::AddBaseline(b) => pr.baselines.push(b),
                    Action::HumanReview | Action::EnableAutoMerge => {
                        panic!("本场景不应出现")
                    }
                },
            }
        }
        assert_eq!(rounds, 6, "3 层 + 2 基准 = 5 次 Hold + 1 次 AutoMerge");
    }
}
