//! 策略引擎的单元测试：把"绿即合永不默认""分层 CI 永不一次性放行"
//! "预测合并要吃完传播后的有效裁决"这三条判定钉住。
//!
//! 从 `policy.rs` 拆出来——测试占了这个模块一半以上，分开之后生产逻辑读起来只剩档位与决策。
//! 子模块用 `use super::*`，可见性与原来完全一致（不放宽任何字段/函数）。

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
    assert!(matches!(
        decide(&MergePolicy::default(), &pr),
        Decision::Block(_)
    ));
}

#[test]
fn propagated_minus_one_vetoes_even_with_allow_gate() {
    // 因果链干净（gate=Allow）、CI 全绿、开关开，但有效裁决因跨仓/坐标污染为 -1
    let p = MergePolicy {
        channel: Channel::Main,
        auto_merge: true,
        required_baselines: vec!["perf-baseline".into()],
    };
    let mut pr = green();
    pr.verdict = Trit::Vulnerable;
    assert!(
        matches!(decide(&p, &pr), Decision::Block(_)),
        "污染后的 -1 必须否决，否则自动合并放行被打穿的依赖"
    );
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
