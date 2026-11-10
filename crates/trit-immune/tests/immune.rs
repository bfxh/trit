//! 免疫网络集成行为：门禁三态、攻击回链、隔离传播、沙盒升级、0 态队列。

use trit_immune::*;

#[test]
fn gate_block_freeze_allow() {
    assert_eq!(
        merge_gate([Trit::Hardened, Trit::Hardened]),
        GateDecision::Allow
    );
    assert_eq!(
        merge_gate([Trit::Hardened, Trit::Unknown]),
        GateDecision::Freeze
    );
    assert_eq!(
        merge_gate([Trit::Unknown, Trit::Vulnerable]),
        GateDecision::Block
    );
    assert_eq!(merge_gate([Trit::Vulnerable]), GateDecision::Block);
}

#[test]
fn fresh_code_is_zero_not_safe() {
    let mut dag = ImmuneDag::root("main");
    let f = dag.spawn(NodeId(0), NodeKind::Delta, "新功能");
    assert_eq!(dag.node(f).verdict, Trit::Unknown);
    // 未测即 0 → 新分支默认冻结，而不是默认放行
    assert_eq!(dag.merge_gate(f), GateDecision::Freeze);
}

#[test]
fn exploit_blocks_then_patch_replay_allows() {
    let mut dag = ImmuneDag::root("main");
    let login = dag.spawn(NodeId(0), NodeKind::Delta, "登录");
    let atk = dag.spawn(login, NodeKind::AttackVector, "SQLi");
    dag.record(
        atk,
        Trit::Vulnerable,
        Some(ExploitEvidence {
            vector: AttackVector::SqlInjection,
            payload: "' OR 1=1 --".into(),
            target: login,
            round: 2,
        }),
        "打穿",
    );
    assert_eq!(dag.merge_gate(login), GateDecision::Block);

    // 负数消息回链：攻击节点持有指向被打穿代码节点的证据
    let ev = dag
        .node(atk)
        .history
        .iter()
        .find_map(|e| e.evidence.as_ref())
        .expect("攻击成功必须带证据");
    assert_eq!(ev.target, login);

    // 补丁 + 重放失败 → 放行
    let patch = dag.spawn(login, NodeKind::Patch, "补丁");
    dag.record(patch, Trit::Hardened, None, "回归");
    dag.record(atk, Trit::Hardened, None, "重放失败");
    dag.record(login, Trit::Hardened, None, "复验");
    assert_eq!(dag.merge_gate(login), GateDecision::Allow);

    // 历史保留完整对抗时间线：攻击成功 → 防御成功
    let hist = &dag.node(atk).history;
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0].verdict, Trit::Vulnerable);
    assert_eq!(hist[1].verdict, Trit::Hardened);
}

#[test]
fn attack_on_sibling_branch_does_not_leak() {
    let mut dag = ImmuneDag::root("main");
    let a = dag.spawn(NodeId(0), NodeKind::Delta, "A");
    let atk = dag.spawn(a, NodeKind::AttackVector, "攻击A");
    dag.record(atk, Trit::Vulnerable, None, "打穿A");
    dag.record(a, Trit::Vulnerable, None, "A 未修复");

    let b = dag.spawn(NodeId(0), NodeKind::Delta, "B");
    dag.record(b, Trit::Hardened, None, "B 通过");

    // A 的漏洞只堵 A 的因果链，不殃及 B
    assert_eq!(dag.merge_gate(a), GateDecision::Block);
    assert_eq!(dag.merge_gate(b), GateDecision::Allow);
}

#[test]
fn quarantine_freezes_until_human_clears() {
    let mut dag = ImmuneDag::root("main");
    let d = dag.spawn(NodeId(0), NodeKind::Delta, "服务");
    dag.record(d, Trit::Hardened, None, "自身通过");
    let m = dag.spawn(d, NodeKind::Message, "外部内容");
    dag.record(m, Trit::Hardened, None, "内容初检通过");
    let g = dag.spawn(m, NodeKind::Delta, "生成代码");
    dag.record(g, Trit::Hardened, None, "回归通过");
    assert_eq!(dag.merge_gate(g), GateDecision::Allow);

    dag.quarantine(m, "注入模式");
    assert!(dag.node(m).quarantined);
    assert!(dag.node(g).quarantined, "隔离沿因果链向后传播");
    assert_eq!(dag.merge_gate(g), GateDecision::Freeze);

    dag.human_review(m, Trit::Hardened, "复核：无注入");
    assert!(!dag.node(g).quarantined, "解除隔离沿子树传播");
    // 隔离只拦"放行"，不抹裁决：原 +1 恢复生效
    assert_eq!(dag.merge_gate(g), GateDecision::Allow);
}

#[test]
fn sandbox_is_deterministic_and_consistent() {
    let sb = Sandbox::new();
    let r1 = sb.fuzz("评论区", "反射输出", AttackVector::Xss, 512);
    let r2 = sb.fuzz("评论区", "反射输出", AttackVector::Xss, 512);
    assert_eq!(r1.verdict, r2.verdict);
    assert_eq!(r1.rounds, r2.rounds);
    // 结论与证据自洽：打穿必有 finding + payload，加固必无
    assert_eq!(r1.finding.is_some(), r1.verdict == Trit::Vulnerable);
    assert_eq!(
        sb.will_break("评论区", "反射输出"),
        r1.verdict == Trit::Vulnerable
    );
    if let Some(v) = &r1.finding {
        assert_eq!(*v, AttackVector::Xss);
        assert!(!r1.payload.as_deref().unwrap_or_default().is_empty());
    }
}

#[test]
fn suspects_queue_lists_unknown_nodes() {
    let mut dag = ImmuneDag::root("main");
    let f = dag.spawn(NodeId(0), NodeKind::Delta, "新功能");
    assert_eq!(dag.suspects(), vec![f]);
    dag.record(f, Trit::Hardened, None, "通过");
    assert!(dag.suspects().is_empty());
}

#[test]
fn convergence_measures_distance_to_fixed_point() {
    let mut dag = ImmuneDag::root("main");
    let a = dag.spawn(NodeId(0), NodeKind::Delta, "A");
    let b = dag.spawn(NodeId(0), NodeKind::Delta, "B");
    let c = dag.spawn(NodeId(0), NodeKind::Delta, "C");
    dag.record(a, Trit::Hardened, None, "通过");
    // b、c 默认 0（未测即不可信）
    let cv = dag.convergence();
    assert_eq!(cv.total, 4);
    assert_eq!(cv.hardened, 2, "root + A");
    assert_eq!(cv.unknown, 2);
    assert_eq!(cv.vulnerable, 0);
    assert!((cv.ratio - 0.5).abs() < 1e-9);

    // 修复 C → 收敛率单向上升
    dag.record(c, Trit::Hardened, None, "通过");
    assert!(dag.convergence().ratio > cv.ratio, "收敛率只许单向改善");

    // 设计层检查：扇入超阈的节点是 0 态候选
    let d = dag.spawn(NodeId(0), NodeKind::Delta, "D");
    let e = dag.spawn(NodeId(0), NodeKind::Delta, "E");
    dag.cross_link(a, d, Trit::Hardened, "A→D");
    dag.cross_link(b, d, Trit::Unknown, "B→D");
    dag.cross_link(c, d, Trit::Hardened, "C→D");
    dag.cross_link(a, e, Trit::Hardened, "A→E");
    assert_eq!(dag.design_smells(2), vec![d], "扇入 > 2 的只有 D");
    assert!(dag.design_smells(3).is_empty());

    // 隔离计数进度量
    dag.quarantine(b, "测试隔离");
    assert_eq!(dag.convergence().quarantined, 1);
}
