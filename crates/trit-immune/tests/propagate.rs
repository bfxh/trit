//! 传播算法与门禁的集成行为：依赖边传染、隔离源跨树传播、源修复恢复下游。

use trit_immune::*;

/// 节点布局：#0 main，#1 登录，#2 攻击，#3 支付（依赖登录）。
fn dag_with_dependency(pass: Trit) -> ImmuneDag {
    let mut dag = ImmuneDag::root("main");
    let login = dag.spawn(NodeId(0), NodeKind::Delta, "登录");
    let atk = dag.spawn(login, NodeKind::AttackVector, "SQLi");
    dag.record(atk, Trit::Vulnerable, None, "打穿");
    dag.record(login, Trit::Vulnerable, None, "攻击面已证伪");
    let pay = dag.spawn(NodeId(0), NodeKind::Delta, "支付");
    dag.record(pay, Trit::Hardened, None, "自身回归通过");
    dag.cross_link(login, pay, pass, "支付复用登录的会话解析");
    dag
}

#[test]
fn confirmed_dependency_blocks_dependent_branch() {
    // 支付自身 +1，但依赖的登录被证伪且依赖边确认可穿透 → 传染为 -1 → BLOCK
    assert_eq!(
        dag_with_dependency(Trit::Vulnerable).merge_gate(NodeId(3)),
        GateDecision::Block
    );
}

#[test]
fn suspect_dependency_freezes_dependent_branch() {
    assert_eq!(
        dag_with_dependency(Trit::Unknown).merge_gate(NodeId(3)),
        GateDecision::Freeze
    );
}

#[test]
fn blocked_dependency_does_not_spread() {
    // 依赖边已证拦截（如已加 WAF）→ 不传染 → 放行
    assert_eq!(
        dag_with_dependency(Trit::Hardened).merge_gate(NodeId(3)),
        GateDecision::Allow
    );
}

#[test]
fn patching_the_source_restores_dependents() {
    let mut dag = dag_with_dependency(Trit::Vulnerable);
    assert_eq!(dag.merge_gate(NodeId(3)), GateDecision::Block);
    dag.record(NodeId(2), Trit::Hardened, None, "重放失败"); // 攻击分支
    dag.record(NodeId(1), Trit::Hardened, None, "补丁复验"); // 登录
    assert_eq!(dag.merge_gate(NodeId(3)), GateDecision::Allow);
}

#[test]
fn quarantined_message_poisons_linked_branch() {
    let mut dag = ImmuneDag::root("main");
    let docs = dag.spawn(NodeId(0), NodeKind::Delta, "文档服务");
    let page = dag.spawn(docs, NodeKind::Message, "外部网页");
    dag.quarantine(page, "Prompt 注入模式");
    let pay = dag.spawn(NodeId(0), NodeKind::Delta, "支付");
    dag.record(pay, Trit::Hardened, None, "自身通过");

    // 没有跨树边：隔离不殃及别的分支
    assert_eq!(dag.merge_gate(pay), GateDecision::Allow);

    // 声明"支付也渲染了这条被污染的消息"（疑似穿透）→ 0 强度传染 → FREEZE
    dag.cross_link(page, pay, Trit::Vulnerable, "支付复用同一消息渲染器");
    assert_eq!(dag.merge_gate(pay), GateDecision::Freeze);

    // 人工复核解除隔离 → 污染源消失 → 恢复放行
    dag.human_review(page, Trit::Hardened, "复核：无实际注入");
    assert_eq!(dag.merge_gate(pay), GateDecision::Allow);
}

#[test]
fn no_cross_link_no_cross_tree_leak() {
    // 兄弟分支的确证漏洞在没有依赖边时不传染（树内因果链已由 chain 覆盖）
    let mut dag = ImmuneDag::root("main");
    let a = dag.spawn(NodeId(0), NodeKind::Delta, "A");
    let atk = dag.spawn(a, NodeKind::AttackVector, "攻击A");
    dag.record(atk, Trit::Vulnerable, None, "打穿A");
    dag.record(a, Trit::Vulnerable, None, "A 未修复");
    let b = dag.spawn(NodeId(0), NodeKind::Delta, "B");
    dag.record(b, Trit::Hardened, None, "B 通过");
    assert_eq!(dag.merge_gate(b), GateDecision::Allow);
    assert_eq!(dag.effective_verdicts()[b.0 as usize], Trit::Hardened);
}
