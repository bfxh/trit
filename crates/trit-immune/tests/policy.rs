#![cfg(feature = "full")]
//! 预测合并端到端：免疫网络（门禁 + 传播）作为预测器，喂给策略引擎。

use std::net::SocketAddr;
use trit_immune::*;

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("trit-immune-{name}-{}.journal", std::process::id()))
}

/// 预测合并 = ImmuneDag::merge_gate（因果链 + 传播后有效裁决）作为快照输入。
fn predict(dag: &ImmuneDag, tip: NodeId) -> GateDecision {
    dag.merge_gate(tip)
}

#[test]
fn propagator_feeds_policy_end_to_end() {
    let mut dag = ImmuneDag::root("main");
    let login = dag.spawn(NodeId(0), NodeKind::Delta, "登录");
    let pay = dag.spawn(NodeId(0), NodeKind::Delta, "支付");
    dag.record(pay, Trit::Hardened, None, "自身通过");
    dag.cross_link(login, pay, Trit::Vulnerable, "支付复用登录会话解析");

    // 上游尚未出结论：预测为 FREEZE → 策略 Hold（去跑 CI 第 1 层）
    let gate = predict(&dag, pay);
    assert_eq!(gate, GateDecision::Freeze);
    let snapshot = PrSnapshot {
        gate,
        layers: vec![],
        baselines: vec![],
        verdict: dag.effective_verdicts()[pay.0 as usize],
    };
    assert!(matches!(
        decide(&MergePolicy::default(), &snapshot),
        Decision::Hold { .. }
    ));

    // 上游被证伪：门禁 BLOCK → 策略一票否决（哪怕 CI 全绿）
    let atk = dag.spawn(login, NodeKind::AttackVector, "SQLi");
    dag.record(atk, Trit::Vulnerable, None, "打穿");
    dag.record(login, Trit::Vulnerable, None, "面已证伪");
    let gate = predict(&dag, pay);
    assert_eq!(gate, GateDecision::Block);
    let snapshot = PrSnapshot {
        gate,
        layers: vec![Trit::Hardened, Trit::Hardened],
        baselines: vec![],
        verdict: dag.effective_verdicts()[pay.0 as usize],
    };
    assert!(matches!(
        decide(&MergePolicy::default(), &snapshot),
        Decision::Block(_)
    ));

    // 修复后传播恢复：门禁 ALLOW + CI 全绿 + 开关开 → 自动合并
    dag.record(atk, Trit::Hardened, None, "重放失败");
    dag.record(login, Trit::Hardened, None, "补丁复验");
    let gate = predict(&dag, pay);
    assert_eq!(gate, GateDecision::Allow);
    let policy = MergePolicy {
        channel: Channel::Main,
        auto_merge: true,
        required_baselines: vec![],
    };
    let snapshot = PrSnapshot {
        gate,
        layers: vec![Trit::Hardened, Trit::Hardened],
        baselines: vec![],
        verdict: dag.effective_verdicts()[pay.0 as usize],
    };
    assert_eq!(decide(&policy, &snapshot), Decision::AutoMerge);
}

#[test]
fn coupling_reports_fan_and_worst_edges() {
    let mut dag = ImmuneDag::root("main");
    let a = dag.spawn(NodeId(0), NodeKind::Delta, "A");
    let b = dag.spawn(NodeId(0), NodeKind::Delta, "B");
    let c = dag.spawn(NodeId(0), NodeKind::Delta, "C");
    dag.cross_link(a, b, Trit::Vulnerable, "A 依赖 B");
    dag.cross_link(c, b, Trit::Unknown, "C 疑似依赖 B");

    let cp = dag.coupling(b);
    assert_eq!(cp.fan_in, 2);
    assert_eq!(cp.fan_out, 0);
    assert_eq!(cp.inbound, Trit::Vulnerable, "入边最坏值：确认能打进来");
    assert_eq!(cp.outbound, Trit::Hardened, "无出边 = 无嫌疑");
    assert_eq!(dag.coupling(a).fan_in, 0);
    assert_eq!(dag.coupling(c).outbound, Trit::Unknown);
}

#[test]
fn server_gate_endpoint_still_works_after_policy_added() {
    // 烟雾测试：策略加入后服务器行为不回归
    let pa = temp("policy-smoke");
    let _ = std::fs::remove_file(&pa);
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let f = j.spawn(NodeId(0), NodeKind::Delta, "功能").unwrap();
    j.record(f, Trit::Hardened, None, "通过").unwrap();
    j.into_dag().unwrap();

    let server = Server::new(Journal::open_as(&pa, 0).unwrap());
    let addr: SocketAddr = server.serve("127.0.0.1:0".parse().unwrap()).unwrap();
    let (s, b) = server::client::get(addr, "/v1/gate?tip=1").unwrap();
    assert_eq!(s, 200);
    assert!(b.contains("gate=ALLOW"));
    let _ = std::fs::remove_file(&pa);
}
